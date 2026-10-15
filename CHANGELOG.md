# Changelog

このファイルは [Keep a Changelog](https://keepachangelog.com/) の分類に近づける。
バージョンは git タグで配る想定（banto ADR-0011）。crates.io は未公開。

## [Unreleased]

### 追加

- Cargo workspace（`banto-opcua-core` / `banto-opcua-server` / `banto-opcua-client`）の足場。
- 基盤実装の ADR-0001（async-opcua 0.19、本クレートは MIT）。
- ループバックのプロセス内テストと、相互接続用の別ワークフロー（中身は TODO）。
- サーバーの `advertised_host`。未指定アドレスで未設定なら起動しない。
- `ServerConfig` の `with_*` 設定メソッドと、接続待ち時間を指定できるクライアント API。
- 接続待ちの時間切れとキャンセルで TCP 接続が閉じることを確かめる回帰テスト。

### 修正

- 接続失敗時にクライアントのイベントループが残らないようにした。
- IPv6 のエンドポイント URL を括弧付きにした。
- OPC UA DateTime の変換を 100 ナノ秒単位で行い、2262 年以降も変換できるようにした。境界テストは `SystemTime` の範囲に依存しない。
- サーバーの Drop が待受タスクを abort し、証明書の一時ディレクトリはタスクが終わるまで残るようにした。
- `advertised_host` に URL、ポート、パスなどを含めた設定を起動前に拒否する。
