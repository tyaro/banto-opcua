# ADR-0002: 動的タグの現在値をプロトコル非依存の入力で渡す

- 状態: Accepted
- 日付: 2026-10-16
- 関連: [ADR-0001](0001-async-opcua-base.md)、[banto-industrial #201](https://github.com/tyaro/banto-industrial/issues/201)

## コンテキスト

固定タグの試験用 API は更新時に現在時刻を付ける。ハブの現在値は値・品質と、
採取時刻・品質認識時刻を別々に持つため、試験用 API では SourceTimestamp を
正しく公開できない。動的タグの追加・削除は既に実装したが、実データの登録と
更新には公開 API がない。サーバーの公開 API に async-opcua の `DataValue` を
出すと、ADR-0001 のクレート境界にも反する。

## 決定

サーバーへ `TagReading`（DataType、`Option<f64>`、品質、`SampleTimestamps`）
を渡す `add_tag_reading` / `update_tag_reading` を設ける。値と StatusCode は
core の写像を使い、時刻は呼び出し側が選んだものだけを変換する。未取得でも
Boolean / Double の DataType を登録時に固定し、値なし・Bad を公開できる。
更新はノードの型確認・値の保存・購読通知を同じアドレス空間の排他区間で行う。

Phase 1 のサンプルタグは既定で残し、ハブは `with_sample_tags(false)` で外す。
これにより実タグがサンプルと同じ外部名でも衝突しない。

## 検討した代替案

- **採用: core の語彙を渡し、サーバー境界で DataValue に変換する。** ハブは
  async-opcua に依存せず、品質と時刻の既存契約を共有できる。ハブ側には
  タグレジストリの照合と周期更新の責任が残る。
- **不採用: ハブが `DataValue` を組み立てて渡す。** プロトコル型がハブへ漏れ、
  `Variant::Empty` による値なし表現を各利用者へ重複させる。
- **不採用: サーバーがハブの値ストアを直接読む。** 汎用ライブラリにハブ固有の
  依存とライフサイクルを持ち込み、Stale の時間経過やタグ変更の責任が曖昧になる。

## 帰結

ハブは更新・品質変化を定期的にサーバーへ反映し、削除・リネーム・型変更は
削除と再登録で表す。SourceTimestamp が不明なときは現在時刻で補わない。
公開範囲を選ぶハブ側の設定では、サンプルタグを無効にする必要がある。
