# Architecture Decision Records (ADR)

作成日: 2026-10-15

このディレクトリは banto-opcua の基盤判断を、代替案の比較込みで記録する。
書式は [banto の ADR テンプレート](https://github.com/tyaro/banto/blob/main/docs/adr/0000-template.md)
に合わせる。英語版（`.en.md`）は banto 本体がテンプレートとして二言語を
持っているためのもので、このリポジトリの文書は日本語だけにする。

## なぜ ADR か

「なぜ」の置き場は banto と同じく分ける。

| 置き場 | 何を書くか |
| --- | --- |
| コードコメント | その場でしか意味を持たない理由 |
| [conventions.md](../conventions.md) | 守る不変条件 |
| ADR（本ディレクトリ） | 代替案を比較して 1 つを選んだ判断 |

## 運用

- ファイル名は `NNNN-kebab-title.md`。書式は [0000-template.md](0000-template.md)。
- 過去の ADR は書き換えない。覆すときは新しい ADR で supersede する。
- 状態は `Proposed` / `Accepted` / `Superseded by ADR-NNNN` / `Deprecated`。

## 索引

| # | タイトル | 状態 |
| --- | --- | --- |
| [0001](0001-async-opcua-base.md) | OPC UA の基盤実装に async-opcua（MPL-2.0）を採用し、本クレート自体は MIT のままにする | Accepted |
| [0002](0002-dynamic-reading-boundary.md) | 動的タグの現在値をプロトコル非依存の入力で渡す | Accepted |
| [0003](0003-persistent-pki-dir.md) | 自己署名の鍵と証明書の保存先だけを Phase 3 より前に設定可能にする | Accepted |
| [0004](0004-write-through-host-handler.md) | OPC UA の Write を利用者が渡す書き込みハンドラーに通す | Accepted |
