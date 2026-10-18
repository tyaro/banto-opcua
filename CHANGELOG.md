# Changelog

このファイルは [Keep a Changelog](https://keepachangelog.com/) の分類に近づける。
バージョンは git タグで配る（banto ADR-0011）。crates.io は未公開。

## [Unreleased]

### 追加

- core の `NodeAddress`。クライアントが読み書きするノードの文字列表現（`ns=` / `nsu=` と `i=` / `s=`、async-opcua の表示が付ける `svr=0;`）を解析し、接続先の NamespaceArray で `NodeId` へ解決する。GUID と ByteString の識別子は「未対応の識別子の型」として、ほかの解析エラーと区別する（ADR-0005、#33）。
- core の `ScalarValue` / `ScalarType`。クライアントが読み書きする OPC UA のスカラーの組み込み型（ADR-0005、#33）。
- クライアントの `ClientConfig` と `OpcUaClient::connect_with`。接続の待ち時間・要求の時間切れ・keep-alive の間隔と失敗回数を設定し、既定では async-opcua の自動再接続を止める。`ReconnectPolicy::Automatic` で部品の側の自動再接続も選べる（張り直した後に NamespaceArray と OperationLimits を読み直す）（ADR-0005、#33）。
- クライアントの `ConnectedClient::read_batch`。ノードの並びをまとめて読み、値・StatusCode・SourceTimestamp をノードごとに返す。`MaxNodesPerRead` で分けて送り、`BadTooManyOperations` では半分にして送り直す。`nsu=` は接続ごとに読んだ NamespaceArray で解決する（ADR-0005、#33）。
- クライアントの `ConnectedClient::write_batch`。ノードと値の並びをまとめて書き、ノードごとの StatusCode を返す。`MaxNodesPerWrite` で分けて送り、`BadTooManyOperations` では半分にして送り直す。接続ごとの失敗では `WriteFailure` に、ノードごとの結末（`WriteOutcome`: 書いた結果・結果不明・拒否・送っていない）を要求の順に持たせる（ADR-0005、#33）。
- クライアントの `ClientConfig::with_default_max_nodes_per_write`（サーバーが上限を広告しないとき。既定 10）。
- サーバーの `ServerConfig::with_max_nodes_per_read`（既定 10000）。
- core の `system_time_from_opcua_ticks` と、StatusCode の `BAD_NOT_SUPPORTED` / `BAD_TOO_MANY_OPERATIONS` / `BAD_UNKNOWN_RESPONSE` / `BAD_CONNECTION_CLOSED`。
- 依存に `futures-util`（async-opcua が既に引いている版。クライアントのイベントループを回す）。

### 変更

- 既存の `OpcUaClient::connect` / `connect_with_session_timeout` も、async-opcua の自動再接続を止めるようになった（以前は 2 回まで張り直した）。
- `ConnectedClient::read_server_state` / `read_u32` は、接続が切れている（張り直している）ときに `Error::Disconnected` を返すようになった（以前は `Error::Read`）。

## [0.1.0] - 2026-10-17

最初のタグ。Phase 1 の固定タグ、Phase 2 の動的タグと品質・時刻の写像、
ADR-0004 の書き込みハンドラーに通す Write を含む。

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
- 動的タグの型・値・品質・時刻を受け取る `TagReading` と `add_tag_reading` / `update_tag_reading`。未取得時も DataType を保持し、Read / Subscription に既知の時刻だけを公開する。
- `ServerConfig::with_sample_tags(false)`。Phase 1 の固定サンプルを外して実タグの外部名との衝突を避ける。
- ドットを含む接続・グループ・タグ名と、ルートと同名の接続 `Banto` を Browse と外部名 NodeId で扱えるようにした。
- `ServerConfig::with_pki_dir`。自己署名の鍵と証明書を初回だけ作って再利用し、2 回目以降の起動を速くする（ADR-0003）。
- 非有限値が購読でも値なし・Bad と採取時刻で届くことを確かめるテスト。
- 利用者の書き込みハンドラーに通す Write（ADR-0004）。`OpcUaServer::start_with_write_handler`、`WriteHandler`、`RunningServer::set_tag_writable` と、core の `WriteRequest`・`WritePrincipal`・`WriteClient`・`WriteRejection`・`status_code_for_write_rejection`。ハンドラーを渡さないサーバーは読み取り専用のまま。
- `ServerConfig::with_max_nodes_per_write`（既定 10）と `with_max_concurrent_writes`（既定 8）。上限を超えた Write は `BadTooManyOperations`、同時実行数の上限のノードは `BadResourceUnavailable`。
- core の `StatusCode` に書き込み用の定数（`BAD_NOT_WRITABLE` など 8 つ）。
- Python asyncua からの Write を相互接続テストに追加した。

### 修正

- CloseSession を送らずに TCP だけが切れたとき、複数ノードの Write の残りのノードを書かないようにした。接続の終了時にサービスの呼び出しを中止する fork の修正（tyaro/async-opcua#2）を固定した。
- sampling interval が -1 の購読で、周期前の中間値を余分に通知しない fork の修正を固定した。
- 接続失敗時にクライアントのイベントループが残らないようにした。
- IPv6 のエンドポイント URL を括弧付きにした。
- OPC UA DateTime の変換を 100 ナノ秒単位で行い、2262 年以降も変換できるようにした。境界テストは `SystemTime` の範囲に依存しない。
- サーバーの Drop が待受タスクを abort し、証明書の一時ディレクトリはタスクが終わるまで残るようにした。
- `advertised_host` に URL、ポート、パスなどを含めた設定を起動前に拒否する。
- `advertised_host` に末尾ドット付きの DNS 名を指定でき、URL にはドットなしで載せる。

### 変更

- タグの node manager を async-opcua の `SimpleNodeManager` から自前の実装に置き換えた。読み取りと購読の挙動は変わらない。
- 最小 sampling interval を 0 ms とし、0 ms を要求した購読が実際に 0 ms で作成されるようにした。非ゼロで遅れた通知も採取時刻を保持する async-opcua の MPL-2.0 fork を固定した。
- ドット入りの名前で NodeId が一致しても、登録時と異なるグループからの更新・削除を拒否する。
- `ServerConfig` は外部クレートで構造体リテラルを使えなくなった。`ServerConfig::default().with_port(...)` などへ移行する。
- 自由関数 `endpoint_url` を公開 API から外した。`ServerConfig::endpoint_url` を使う。
- `RunningServer::namespace_index()` を追加。`Error::TagUpdate` の表示文言をタグ操作全般に合わせた。

[Unreleased]: https://github.com/tyaro/banto-opcua/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/tyaro/banto-opcua/releases/tag/v0.1.0
