# AGENTS.md

banto-opcua で作業するエージェント向けの索引。規約の本文は各文書にある。

- [README.md](README.md) — スコープ、クレート、テストの層、未決事項
- [docs/conventions.md](docs/conventions.md) — 不変条件
- [docs/adr/0001-async-opcua-base.md](docs/adr/0001-async-opcua-base.md) — 基盤実装と MIT / MPL-2.0
- [CLAUDE.md](CLAUDE.md) — コミットと文書の流儀
- 上流の流儀: [tyaro/banto](https://github.com/tyaro/banto) の `docs/conventions.md` と ADR、[tyaro/banto-industrial](https://github.com/tyaro/banto-industrial) の CI・`deny.toml`・ツールチェイン

## 触るときの注意

- プロトコルの実装を自前で書き始めない。基盤は ADR-0001 の async-opcua。
- `banto-opcua-core` に async-opcua を依存させない。MPL の型を公開 API に出さない。
- MPL のソースをこのリポジトリのファイルへコピーしない。
- 既定 bind は `127.0.0.1`。Phase 1 は SecurityPolicy `None` と Anonymous。
- コメントと文書は日本語。識別子は英語のまま。
- Rust の実装時は [.cursor/rules/pragmatic-rust-guidelines.mdc](.cursor/rules/pragmatic-rust-guidelines.mdc) を参照する。既存の banto の流儀と衝突したら既存流儀を優先し、理由を ADR かモジュール doc に残す。
