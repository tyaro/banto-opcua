# Changelog

このファイルは [Keep a Changelog](https://keepachangelog.com/) の分類に近づける。
バージョンは git タグで配る想定（banto ADR-0011）。crates.io は未公開。

## [Unreleased]

### 追加

- Cargo workspace（`banto-opcua-core` / `banto-opcua-server` / `banto-opcua-client`）の足場。
- 基盤実装の ADR-0001（async-opcua 0.19、本クレートは MIT）。
- ループバックのプロセス内テストと、相互接続用の別ワークフロー（中身は TODO）。
