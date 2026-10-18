//! サーバーの上限に合わせて要求を分けて送る（ADR-0005）。

use std::future::Future;

/// 分けて送った要求が止まった理由。
#[derive(Debug, PartialEq)]
pub(crate) enum ChunkFailure<E> {
    /// 塊の要求が失敗した。`chunk_len` はその塊のノード数。
    Request { error: E, chunk_len: usize },
    /// 応答の結果の数が塊のノード数と合わなかった。
    CountMismatch { expected: usize, actual: usize },
}

/// [`send_in_chunks`] の失敗。
#[derive(Debug, PartialEq)]
pub(crate) struct ChunkError<R, E> {
    /// 失敗より前に結果を受け取った分。要求の先頭からの順で、失敗した塊は
    /// `done.len()` 番目から始まる。
    pub(crate) done: Vec<R>,
    pub(crate) failure: ChunkFailure<E>,
}

/// `items` を `chunk_size` 件ずつ `send` へ渡し、結果を要求の順に並べる。
///
/// `too_many` が真を返す失敗（`BadTooManyOperations`）は、サービス全体の拒否で
/// 塊は適用されていない。そのときは塊を半分の大きさにして同じ位置から送り直す。
/// 1 件の塊でも拒否されたら失敗にする。小さくした大きさは `chunk_size` に残り、
/// 呼び出し側は次の要求にも使える。空の `items` は送らずに空の結果を返す。
pub(crate) async fn send_in_chunks<T, R, E, F, Fut>(
    items: &[T],
    chunk_size: &mut usize,
    too_many: impl Fn(&E) -> bool,
    mut send: F,
) -> Result<Vec<R>, ChunkError<R, E>>
where
    T: Clone,
    F: FnMut(Vec<T>) -> Fut,
    Fut: Future<Output = Result<Vec<R>, E>>,
{
    let mut done = Vec::with_capacity(items.len());
    let mut start = 0;
    while start < items.len() {
        let end = start + (*chunk_size).max(1).min(items.len() - start);
        let chunk_len = end - start;
        match send(items[start..end].to_vec()).await {
            Ok(results) if results.len() == chunk_len => {
                done.extend(results);
                start = end;
            }
            Ok(results) => {
                return Err(ChunkError {
                    done,
                    failure: ChunkFailure::CountMismatch {
                        expected: chunk_len,
                        actual: results.len(),
                    },
                });
            }
            Err(error) if chunk_len > 1 && too_many(&error) => *chunk_size = chunk_len / 2,
            Err(error) => {
                return Err(ChunkError {
                    done,
                    failure: ChunkFailure::Request { error, chunk_len },
                });
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    #[derive(Debug, PartialEq)]
    enum Fail {
        TooMany,
        Other,
    }

    /// 受け取った塊の大きさを記録し、`limit` を超える塊を拒否する偽のサーバー。
    async fn run(
        items: usize,
        chunk_size: &mut usize,
        limit: usize,
        fail_at: Option<usize>,
    ) -> (Result<Vec<usize>, ChunkError<usize, Fail>>, Vec<usize>) {
        let sent = RefCell::new(Vec::new());
        let items: Vec<usize> = (0..items).collect();
        let result = send_in_chunks(
            &items,
            chunk_size,
            |error| *error == Fail::TooMany,
            |chunk: Vec<usize>| {
                sent.borrow_mut().push(chunk.len());
                let result = if chunk.len() > limit {
                    Err(Fail::TooMany)
                } else if fail_at.is_some_and(|at| chunk.contains(&at)) {
                    Err(Fail::Other)
                } else {
                    Ok(chunk.iter().map(|item| item * 10).collect())
                };
                async move { result }
            },
        )
        .await;
        (result, sent.into_inner())
    }

    #[tokio::test]
    async fn splits_by_chunk_size_and_keeps_order() {
        let mut size = 2;
        let (result, sent) = run(5, &mut size, usize::MAX, None).await;
        assert_eq!(result, Ok(vec![0, 10, 20, 30, 40]));
        assert_eq!(sent, vec![2, 2, 1]);
    }

    #[tokio::test]
    async fn empty_input_sends_nothing() {
        let mut size = 2;
        let (result, sent) = run(0, &mut size, usize::MAX, None).await;
        assert_eq!(result, Ok(vec![]));
        assert!(sent.is_empty());
    }

    #[tokio::test]
    async fn too_many_operations_halves_and_resends_from_the_rejected_chunk() {
        let mut size = 8;
        let (result, sent) = run(7, &mut size, 3, None).await;
        assert_eq!(result, Ok((0..7).map(|item| item * 10).collect()));
        assert_eq!(sent, vec![7, 3, 3, 1]);
        assert_eq!(size, 3, "小さくした大きさを次の要求に残す");
    }

    #[tokio::test]
    async fn single_node_rejection_fails() {
        let mut size = 4;
        let (result, sent) = run(3, &mut size, 0, None).await;
        assert_eq!(
            result,
            Err(ChunkError {
                done: vec![],
                failure: ChunkFailure::Request {
                    error: Fail::TooMany,
                    chunk_len: 1,
                },
            })
        );
        assert_eq!(sent, vec![3, 1]);
    }

    #[tokio::test]
    async fn other_failure_stops_and_reports_what_was_done() {
        let mut size = 2;
        let (result, sent) = run(6, &mut size, usize::MAX, Some(3)).await;
        assert_eq!(
            result,
            Err(ChunkError {
                done: vec![0, 10],
                failure: ChunkFailure::Request {
                    error: Fail::Other,
                    chunk_len: 2,
                },
            })
        );
        assert_eq!(sent, vec![2, 2]);
    }

    #[tokio::test]
    async fn result_count_mismatch_fails() {
        let items = [1, 2, 3];
        let mut size = 3;
        let result = send_in_chunks(
            &items,
            &mut size,
            |_: &Fail| false,
            |_chunk: Vec<i32>| async { Ok(vec![1]) },
        )
        .await;
        assert_eq!(
            result,
            Err(ChunkError {
                done: vec![],
                failure: ChunkFailure::CountMismatch {
                    expected: 3,
                    actual: 1,
                },
            })
        );
    }
}
