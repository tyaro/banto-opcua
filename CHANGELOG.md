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
- 接続待ちの時間切れとキャンセルで TCP 接続が閉じることを確かめる、対象 PR と nightly の回帰テスト。
- Phase 1 の固定タグ `Speed` / `Running`。Browse / Read / Subscription と試験用の値更新 API、UaExpert 用のサンプルサーバー。
- core の現在値・品質の写像。非有限値、0 / 1 以外の bit、欠損値を値なし・汎用 Bad として扱う。
- core の品質別 SourceTimestamp 写像。時刻不明は null のままにし、固定タグの反復 Read でも採取時刻を保持する。
- 指定 NodeId の UInt32 値を読むクライアント API `read_u32`。
- Python asyncua と opc-plc を使う外部相互接続テスト。固定タグの Browse・Read・購読と外部サーバーの UInt32 タグ Read を検証する。
- フィルタ未指定の購読で品質だけが変わる場合の通知テスト。修正を含む async-opcua の upstream コミットを一時的に固定する。
- `RunningServer::add_tag` / `remove_tag`。削除時に `BadNodeIdUnknown` を通知し、同じ NodeId の再登録で既存の購読を再開する。

### 修正

- 接続失敗時にクライアントのイベントループが残らないようにした。
- IPv6 のエンドポイント URL を括弧付きにした。
- OPC UA DateTime の変換を 100 ナノ秒単位で行い、2262 年以降も変換できるようにした。境界テストは `SystemTime` の範囲に依存しない。
- サーバーの Drop が待受タスクを abort し、証明書の一時ディレクトリはタスクが終わるまで残るようにした。
- `advertised_host` に URL、ポート、パスなどを含めた設定を起動前に拒否する。
- `advertised_host` に末尾ドット付きの DNS 名を指定でき、URL にはドットなしで載せる。

### 変更

- `ServerConfig` は外部クレートで構造体リテラルを使えなくなった。`ServerConfig::default().with_port(...)` などへ移行する。
- 自由関数 `endpoint_url` を公開 API から外した。`ServerConfig::endpoint_url` を使う。
- `RunningServer::namespace_index()` を追加。`Error::TagUpdate` の表示文言をタグ操作全般に合わせた。
