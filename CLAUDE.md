# banto-opcua の開発規約

産業用 OPC UA ライブラリ（Rust workspace）。文書の入口は [README.md](README.md)。
不変条件は [docs/conventions.md](docs/conventions.md)。基盤判断は
[docs/adr/README.md](docs/adr/README.md)。

## 開発規約

- コミットメッセージは日本語の Conventional Commits 風（`feat` / `fix` / `docs` / `chore` / `ci`）。banto / banto-industrial の履歴に合わせる。
- 設計判断は ADR に日付付きで残す。同じ「なぜ」を README と ADR の両方に書かない。
- コードコメントは、型と関数の契約が読み手に伝わる doc コメントを書く。自明な行コメントは足さない。
- 依存を足すときは ADR-0001 と banto ADR-0002 の基準に照らす。足すなら `deny.toml` の許可ライセンスを実測で更新する。
- Markdown の整形に Prettier は使わない（このリポジトリに Node のアプリは無い）。
