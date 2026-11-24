# Changelog

このファイルは [Keep a Changelog](https://keepachangelog.com/) の分類に近づける。
バージョンは git タグで配る（banto ADR-0011）。crates.io は未公開。

## [Unreleased]

## [0.3.3] - 2026-11-24

中身は 0.3.1 と同じ。タグ `v0.3.1`・`v0.3.2` と版がずれたので（下の 0.3.1 の節）、タグ名と版を
そろえ直すために 0.3.3 として配る。取り込む側は `tag = "v0.3.3"` を使う。

### 変更

- workspace の version を 0.3.3 にした。

## [0.3.1] - 2026-11-24

利用者が取り込む crate の中身は 0.3.0 と同じ。v0.3.0 のタグを付けた後にマージした CI の更新を含める。

**この版のタグは使わない。0.3.3 を使う。** タグ `v0.3.1` は、この節を足した #62 のマージより前の
コミット（#53 のマージ `94d0ebf`）に付いてしまい、中身は `version = "0.3.0"` のまま。付け直したタグ
`v0.3.2` は中身が `version = "0.3.1"` で、タグ名と版が合わない。どちらのタグも動かさずに残す。

### 変更

- CI で使う `taiki-e/install-action` を 2.89.1 から 2.90.5 に上げた（#53）。

## [0.3.0] - 2026-11-24

数値と文字列のタグを元の型のまま公開する（ADR-0007、#56）。公開 API に互換でない変更がある。
タグの説明・単位・レンジ・二値の文言を、変数ノードの Description と標準のプロパティで公開する（ADR-0008、#55）。
Phase 3 のセキュリティ（署名・暗号化、UserName、証明書の信頼の一覧）をサーバーとクライアントに入れる（ADR-0009、#54）。サーバーの既定のエンドポイントから `None` が消えた。

### 追加

- core の `DataType` と `MappedValue` に Int16・UInt16・Int32・UInt32・Int64・UInt64・Float・String。サーバーは Variant と DataType 属性をその型のまま作り、Write もその型で受ける。
- core の `publish_data_type(TagType, ValueScaling)`。スケーリングしていない数値は元の型、スケーリング後の工学値は Double、`bit` は Boolean、`string` は String。
- core の `MappedValue::from_number`（f64 から丸めずに変える）、`MappedValue::data_type`、`MappedValue::is_publishable_as`、`map_typed_value`、`MAX_STRING_BYTES`（4096）、`MappedValue` の `Display`。
- core の `From<MappedValue> for ScalarValue` と `TryFrom<ScalarValue> for MappedValue`（SByte と Byte は `NotPublishable`）。
- サーバーの `TagReading::typed`。64 ビットの整数や String を f64 を経由せずに渡す。
- core の `TagMetadata`（`EngineeringUnits`・`EuRange`・`TwoStateLabels`）、`MetadataShape`・`VariableType`・`MetadataError`、`UNECE_UNITS_NAMESPACE_URI`。`EngineeringUnits::unit_id` は UNECE の Common Code から OPC 10000-8 の UnitId を作る。
- サーバーの `RunningServer::add_tag_reading_with_metadata` と `update_tag_metadata`。説明は Description 属性に、単位・レンジ・文言は EngineeringUnits・EURange・TrueState・FalseState のプロパティにする。型定義は、EURange があれば AnalogItemType、単位だけなら BaseAnalogType、文言のある Boolean は TwoStateDiscreteType、ほかは BaseDataVariableType。
- `update_tag_metadata` は、ノードの形（型定義と持つプロパティ）が同じなら同じ NodeId のまま書き換えて購読者へ通知する。単位・レンジ・文言の値が変わったら、タグ本体の Value の監視項目ごとに SemanticsChanged を立てた通知を 1 件送る（保存した値には立てず、通常の Read には出ない）。形が変わるなら `Error::MetadataShapeChanged`（新しい変種）にし、何も変えない。意味を決めるプロパティの AccessLevel には SemanticChange を付ける。
- タグを登録し直したとき、作り直したプロパティの購読者へも通知し、同じ監視項目のまま Good に戻す。
- `remove_tag` はプロパティも消し、プロパティの購読者へも `BadNodeIdUnknown` を通知する。
- クライアントの `ConnectedClient::read_metadata` と `NodeMetadata`。ノードの並びの Description と EngineeringUnits・EURange・TrueState・FalseState のプロパティを読み、`TagMetadata` で返す（型定義は問わない）。Description を持たないノード（`BadAttributeIdInvalid`）からも単位・レンジを読む。Read と TranslateBrowsePathsToNodeIds は `read_batch` と同じ規則で分けて送る。
- core の StatusCode に `BAD_NODE_ID_INVALID` / `BAD_ATTRIBUTE_ID_INVALID`。
- core の `SecurityPolicy`（None・Basic256Sha256・Aes128_Sha256_RsaOaep・Aes256_Sha256_RsaPss）、`MessageSecurityMode`、`UserTokenType`、`Password`（`Debug` に中身を出さない）、`UserNameLogin`、`UserRejection`、`CertificateInfo`。`WritePrincipal::User`。
- 新しいクレート `banto-opcua-pki`。`prepare_own_certificate`（自分の鍵と自己署名の証明書を用意し、解釈できない・鍵と証明書が組にならない・ApplicationUri・ホスト・有効期間が合わないときは作り直す。読めない I/O の失敗では何も消さずに止める。有効期間は 5 年）と、`TrustList`（`trusted/`・`rejected/` の一覧、`trust`・`reject`・`remove`・`add_trusted`・`add_pending`・`state_of`・`migrate`）。一覧のファイル名は拇印だけから作り、両方にある証明書は拒否として扱う。旧い async-opcua が付けた `CN=<CN> [拇印].der` の名前で中身と合うものだけを `migrate` が移し（ほかの名前のファイルは信頼に数えない）、移し先は一時ファイル経由で書いて読み戻してから移し元を消す。
- サーバーの `ServerConfig::with_security_policies`・`with_security_modes`・`with_none_endpoint`・`with_anonymous`・`with_trust_all_client_certificates`。
- サーバーの `OpcUaServer::start_with_handlers` と `ServerHandlers`（書き込みハンドラーと `UserAuthenticator`）。UserName は暗号化したエンドポイントでだけ受け付け、照合を通したセッションの Write は主体が `WritePrincipal::User` になる。照合は 10 秒で打ち切る。
- サーバーの `RunningServer::trust_list`・`certificate`・`certificate_der`・`security_events`（`SecurityEvent`・`SecurityEventKind`）。
- サーバーの `Error::InvalidSecurity`。エンドポイントや認証の組み合わせが成り立たない設定を起動前に断る。
- クライアントの `ClientConfig::with_security`・`with_user_name`・`with_identity`（`ClientIdentity`）・`with_pki_dir`・`with_application`・`with_trust_all_server_certificates`。
- クライアントの `OpcUaClient::endpoints` と `EndpointInfo`（GetEndpoints の SecurityPolicy・MessageSecurityMode・利用者のトークン・サーバーの証明書）。
- クライアントの `Error::EndpointNotFound`・`ServerCertificateUntrusted`・`ServerCertificateInvalid`（`CertificateProblem`）・`ClientCertificateRejected`・`UserRejected`。サーバーの証明書はセッションを開く前に信頼の一覧と照らし、初めて会う証明書は拒否の一覧に置く（保留）。
- 相互接続の Python asyncua の経路で、暗号化したエンドポイントの一覧、違うパスワードの `BadUserAccessDenied`、Basic256Sha256 と Aes256_Sha256_RsaPss での UserName の書き込みを確かめる。

### 変更

- async-opcua の固定を、証明書の保存名を拇印だけにし、IPv6 のホスト名を括弧なし・値で照合する fork の修正（`a08a53a`）へ移した（ADR-0009）。
- `with_trust_all_client_certificates` / `with_trust_all_server_certificates` は試験用として、`pki_dir` と一緒には使えない（本番の一覧の拒否を上書きしない）。
- サーバーの既定のエンドポイントを、Basic256Sha256・Aes128_Sha256_RsaOaep・Aes256_Sha256_RsaPss の Sign と SignAndEncrypt にした。`None` / Anonymous で接続する利用者は、サーバーに `with_none_endpoint(true)` を足す。
- サーバーはクライアントの証明書を全部は信頼せず、`pki_dir` の信頼の一覧で判断する（試験は `with_trust_all_client_certificates(true)`）。`pki_dir` を設定しなければ、一覧も起動ごとに空になる。
- サーバーの鍵と証明書は async-opcua ではなく `banto-opcua-pki` が用意し、SubjectAltName に公開ホストと bind したアドレスを載せる。既存の証明書が ApplicationUri・公開ホスト・有効期間と合わなければ作り直す（拇印が変わる）。生成に失敗したときの `Error::Pki` の `io::ErrorKind` は、失敗の理由のものになる。
- クライアントはサーバーの証明書の検査を止めなくなった（`None` では従来どおり検査しない）。`None` のとき自分の鍵を作らないので、接続が速くなる。接続は GetEndpoints の後、設定に合うエンドポイントへ直接つなぐ。
- `Error::Pki` の説明を、鍵と証明書、信頼の一覧の置き場のものにした。
- async-opcua の固定を、fork の 4 コミットを上流の master（`ca18bb73`）に載せ直し、キューがあふれたり、保留した値が置き換わったりしても SemanticsChanged を次の通知へ引き継ぐ修正を重ねた `c0de87b` へ移した（ADR-0001、#59）。上流の暗号まわりの依存の更新で `rsa` は 0.10.0 になり、RUSTSEC-2023-0071 の ignore と、使われなくなった BSD-2-Clause・BSD-3-Clause の許可を `deny.toml` から外した。
- `publish_decision` と `PublishDecision` を廃止した。`publish_data_type` に置き換える。`string` のタグも String で公開する。
- `TagReading::value` を `Option<f64>` から `Option<MappedValue>` にした。`TagReading::new` は `Option<f64>` を受けたまま `from_number` で変える（`const fn` ではなくなった）。
- `MappedValue`・`MappedReading`・`TagReading` は `Copy` ではなくなった（String を持つため）。
- `DataType` と `MappedValue` を網羅的に `match` していた利用者は、新しい型の腕が要る。
- f64 から整数の型への写像は、範囲外と小数を値なし・汎用 Bad にする。Float は f32 で正確に表せる値だけを受け、0.1 のように丸まる値や 0 に落ちる値は値なし・汎用 Bad にする。Write では、非有限の Float と上限を超える String を `BadOutOfRange`、ノードと違う型を `BadTypeMismatch` にする。

## [0.2.0] - 2026-11-05

Hub のドライバ向けの OPC UA クライアントの部品（ADR-0005、#33）と、購読モード
（ADR-0006）を含む。async-opcua の固定を上流の master の上へ移した（#50、#17）。

### 追加

- クライアントの購読モード（ADR-0006）。`ConnectedClient::monitor` でノードの並びを登録し、`ConnectedClient::latest` で購読で受けた最新の値・StatusCode・SourceTimestamp を登録した順に取り出す。設定は `ClientConfig::with_subscription`（`SubscriptionSettings`）。MonitoredItem の作成は `MaxMonitoredItemsPerCall` で分け、購読に入れられる数を超えたノードは `BadTooManyMonitoredItems` にする。Publish の失敗では購読を作り直し、購読を失ったら（`BadNoSubscription` など）接続ごとの失敗にする。async-opcua の購読の作り直しは使わず、自動再接続の後は NamespaceArray を読み直して部品が作り直す。
- core の StatusCode に `BAD_WAITING_FOR_INITIAL_DATA` / `BAD_TOO_MANY_MONITORED_ITEMS`。
- opc-plc との相互接続テストを、クライアントの部品のまとめて Read（型ごと、`ns=` / `nsu=`、存在しないノード）、まとめて Write（書けるノード、`BadNotWritable`、`BadTypeMismatch`）と Write の後の Read、購読モードへ広げた（例 `interop_opc_plc`。`interop_read` を置き換え）。
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
- クライアントの `Error` を `#[non_exhaustive]` にし、`Disconnected` / `Subscribe` / `Write` を足した。網羅的に `match` していた利用者は、ワイルドカードの腕が要る。
- `ConnectedClient::read_server_state` / `read_u32` は、接続が切れている（張り直している）ときに `Error::Disconnected` を返すようになった（以前は `Error::Read`）。
- async-opcua の固定を、fork の修正 3 件を上流の master（`f2265a91`）の上に載せ直し、Republish の番号の控えが同じ ID の新しい購読に残る不具合の修正を重ねた `9daa957` へ移した。上流の再接続後の Publish の再開と、失われた通知の Republish が入る。依存の `serde_yaml` は `yaml_serde` に替わった（ADR-0001、ADR-0006、#17）。

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

[Unreleased]: https://github.com/tyaro/banto-opcua/compare/v0.3.3...HEAD
[0.3.3]: https://github.com/tyaro/banto-opcua/compare/v0.3.2...v0.3.3
[0.3.1]: https://github.com/tyaro/banto-opcua/compare/v0.3.0...v0.3.2
[0.3.0]: https://github.com/tyaro/banto-opcua/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/tyaro/banto-opcua/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/tyaro/banto-opcua/releases/tag/v0.1.0
