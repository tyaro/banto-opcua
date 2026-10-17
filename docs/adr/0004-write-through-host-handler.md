# ADR-0004: OPC UA の Write を利用者が渡す書き込みハンドラーに通す

- 状態: Accepted
- 日付: 2026-10-17
- 関連: [conventions.md §2](../conventions.md#2-書き込みは利用者の書き込み経路を通す)、[ADR-0001](0001-async-opcua-base.md)、[ADR-0002](0002-dynamic-reading-boundary.md)、[#25](https://github.com/tyaro/banto-opcua/issues/25)、[banto-industrial #653](https://github.com/tyaro/banto-industrial/issues/653)、[banto-industrial #201](https://github.com/tyaro/banto-industrial/issues/201)

## コンテキスト

これまでサーバーは読み取り専用で、変数の AccessLevel は CurrentRead だけだった。
conventions は「書き込みの入口を足すときは、banto-hub の認可と監査を経由する
設計を先に ADR にする」としていた。

2026-10-17 のオーナー決定で、banto-hub における OPC UA サーバーの位置づけが
決まった。

- Hub のタグを外部へ出すインターフェースとして、REST・gRPC・MCP と同列に扱う。
  書き込みの口は REST・gRPC・OPC UA・MCP で、WS と MQTT は書き込みに向かない
  ので持たない。
- OPC UA からの書き込みは既定で許可し、匿名でも書ける。読み取り専用が既定では
  現場で使いにくい。
- OPC UA クライアントは SLMP・Modbus と同列のドライバで、この ADR の範囲外。

Hub には、REST・gRPC・MCP が共通で呼ぶ書き込み関数（`execute_write`）がある。
タグの `writable`、全体の書き込みスイッチ、回数制限、型の確認、書く前の監査は
そこで行う。OPC UA もここを通せば、口ごとに安全策を作り直さずに済む。

基盤の async-opcua には次の性質がある（当時固定していた `60c4f9d` で確認）。

- `SimpleNodeManager::add_write_callback` のコールバックは同期関数で、
  アドレス空間の書き込みロックを持ったまま呼ばれる。書いた主体（セッションの
  ユーザートークン）も受け取らない。PLC の応答を待つ書き込みには使えない。
- `InMemoryNodeManagerImpl` を自前で実装すると、`write` は async になり、
  `RequestContext` からユーザートークンとセッションを取れる。読み取りと購読は
  トレイトの既定実装が受け持ち、本リポジトリはもともと読み取りコールバックも
  サンプラーも使っていない。
- AccessLevel の確認は node manager の `write` の責任で、`validate_node_write` を
  使うと UserAccessLevel に CurrentWrite が無い Value への Write は
  `BadUserAccessDenied` になる。
- SecurityPolicy `None` のエンドポイントでは、UserName トークンのパスワードは
  既定で平文になる。暗号化には RSA の秘密鍵操作が要り、ADR-0001 で無視している
  RUSTSEC-2023-0071 に触れる。
- Python asyncua（`interop` で固定している 1.1.8）の `write_value` は、値に
  SourceTimestamp を付けて送る。

## 決定

サーバーは、利用者が渡す書き込みハンドラーを通してだけ Write を受け付ける。
不変条件は [conventions.md §2](../conventions.md#2-書き込みは利用者の書き込み経路を通す)。

### ハンドラーと主体

- ハンドラーは async のトレイトで、入出力の型は core の語彙で定義する。
  async-opcua の型は公開 API に出さない（ADR-0001、ADR-0002 と同じ境界）。
  トレイト自体は server クレートに置き、core に依存を増やさない。
- ハンドラーに渡すもの: 対象タグの外部名（接続・グループ・タグ）、値
  （`Boolean` または `Double`）、書いた主体、接続元の情報。
- 接続元の情報は、async-opcua のセッションから取れるものに限る。エンドポイント
  URL、セッション ID と名前、クライアントの ApplicationUri と ApplicationName。
  接続元の IP アドレスはセッションに伝わっていないので渡さない。
- 主体は列挙型にする。今は `Anonymous` だけを作り、UserName や証明書の主体は
  Phase 3 で足す。匿名を許すかどうかはハンドラーが決める。サーバーは主体で
  書き込みを止めない。
- ハンドラーを渡さないときは、従来どおり読み取り専用にする。ライブラリとしての
  既定は読み取り専用で、Hub は常にハンドラーを渡すので Hub では既定で書ける。

### ノードと値

- タグの登録・更新で、書き込み可否（`writable`）を受け取る。書けるタグだけ
  AccessLevel と UserAccessLevel に CurrentWrite を付ける。サンプルタグと、
  ハンドラーが無いときのタグは CurrentRead のままにする。
- 受け付けるのは Value 属性だけ。値の型はノードの DataType と一致させる
  （Boolean のノードは Boolean、Double のノードは Double）。型の変換はしない。
- 書き込む DataValue の StatusCode は、無いか Good のときだけ受け付ける。
  Bad や Uncertain は `BadWriteNotSupported` にする。SourceTimestamp と
  ServerTimestamp は受け付けて捨て、ハンドラーには渡さない。ノードの時刻は
  これまでどおり `update_tag_reading` で決まる。
- IndexRange 付きの Write は `BadIndexRangeNoData` にする。公開する変数は
  スカラーだけで、async-opcua 自身もスカラーへの範囲書き込み
  （`Variant::set_range_of`）を `BadIndexRangeNoData` にしている。
- Write ではノードの値を書き換えない。PLC へ届いた値は、いつもどおり
  `update_tag_reading` で戻ってくる。確定していない値を Read や購読に見せない。

### 実行

- Write の応答は、ハンドラーの結果を待ってから返す。結果を後から通知する形には
  しない。アドレス空間のロックを持ったまま待たない。
- ハンドラーの呼び出しは、ノード 1 件ごとに独立したタスクで実行する。
  クライアントが切断して要求が捨てられても、走っている 1 件は最後まで走る
  （利用者の監査が途中で切れない）。まだ始めていない残りのノードは書かない。
  ハンドラーの future は `Send + 'static` にする。
- サーバー側では待つ上限を設けず、時間切れの結果も返さない。上限を設けて
  `BadTimeout` を返すと、PLC への書き込みが続いているのにクライアントへ再送を
  促し、同じ書き込みが二重に届く。ハンドラーは有限の時間で結果を返す契約にする
  （Hub は PLC との通信の時間切れで有限になる）。
- 1 回の Write に複数のノードがあれば、要求の順に 1 件ずつハンドラーを呼び、
  結果をノードごとに返す（Part 4 の Write はノードごとに結果を返す）。全件を
  まとめて成功・失敗にはしない。
- 1 回の Write で受け付けるノード数の上限を、サーバーの設定で小さく保つ
  （async-opcua の既定 10,000 を使わない）。1 件ずつ PLC の応答を待つので、
  上限が大きいと 1 要求で長く占有できてしまう。既定値は #25 で決める。
- サーバー全体で同時に走るハンドラーの数にも上限を設ける（設定、既定値は #25
  で決める）。上限に達しているときは、ハンドラーを呼ばずにそのノードを
  `BadResourceUnavailable` にする。待ち行列にはしない。要求を投げては切断する
  クライアントがいても、PLC の応答待ちのタスクが積み上がらない。
- ハンドラーがあり、ループバック以外で待ち受けるときは、匿名の書き込みが
  外から届く。起動時に警告ログを出す。

### StatusCode

サーバー自身が拒否するものと、ハンドラーが返す拒否の理由を分ける。

サーバーが返すもの（ハンドラーは呼ばない）:

| 状況 | StatusCode |
| --- | --- |
| ハンドラーが無い、または AccessLevel に CurrentWrite が無い | `BadNotWritable` |
| Value 以外の属性 | `BadNotWritable` |
| 値の型がノードの DataType と違う | `BadTypeMismatch` |
| Double が非有限値（NaN、±∞） | `BadOutOfRange` |
| StatusCode が Bad / Uncertain | `BadWriteNotSupported` |
| IndexRange 付き | `BadIndexRangeNoData` |
| 同時に走るハンドラーの数が上限 | `BadResourceUnavailable` |

AccessLevel の確認は `validate_node_write` より前に自前で行う。AccessLevel 由来は
`BadNotWritable`、UserAccessLevel（主体）由来は `BadUserAccessDenied` という
Part 4 の使い分けに合わせるため。

ハンドラーが返す拒否の理由（core で定義する）と StatusCode:

| 拒否の理由 | StatusCode | Hub の例 |
| --- | --- | --- |
| タグが無い | `BadNodeIdUnknown` | `not_found` |
| 書けないタグ | `BadNotWritable` | `not_writable`、`write_unsupported_protocol` |
| 主体に権限が無い | `BadUserAccessDenied` | （Phase 3 の主体） |
| 型が合わない | `BadTypeMismatch` | `unsupported_value_type` |
| 値が範囲外 | `BadOutOfRange` | `value_out_of_range`（整数のタグへの小数を含む） |
| 書き込みを受け付けていない | `BadOutOfService` | `writes_disabled`、`collection_not_running`、`tag_disabled` |
| 回数制限 | `BadRequestNotAllowed` | `rate_limited` |
| 機器との通信に失敗 | `BadCommunicationError` | `write_failed` |
| 内部エラー | `BadInternalError` | `audit_write_failed`、`invalid_address`、`internal` |

回数制限に `BadTooManyOperations` は使わない。Part 4 ではサービス全体の結果で、
要求の操作数が多すぎるという意味になり、クライアントが要求を分けて再送する。

「書けないタグ」と「型が合わない」の多くはサーバーが先に拒否する。ハンドラーの
行は、Hub の `writable` とノードの AccessLevel の同期がずれた瞬間のような、
防御のための写像である。

### 実装方法

- `InMemoryNodeManagerImpl` を自前で実装し、`init`・`name`・`namespaces`・
  `write` だけを持つ。読み取りと購読はトレイトの既定実装に任せる（今の
  `SimpleNodeManager` と同じ経路）。値の更新は今と同じく `InMemoryNodeManager` の
  `set_value` と購読への通知で行う。
- 先に小さな試作で、読み取り・購読の既存テストを保ったまま async の Write と
  ユーザートークンを扱えるかを確かめる。だめなら、fork（tyaro/async-opcua）で
  コールバックに `RequestContext` を渡す修正を検討し、この ADR に追記する。
- 実装（2026-10-17、#25）: 上限の既定値は、1 回の Write のノード数を 10、
  同時に走るハンドラーの数を 8 にした（`ServerConfig` の `max_nodes_per_write`・
  `max_concurrent_writes`）。UaExpert などの手動の書き込みは 1 ノードで、Hub の
  書き込みは機器ごとに順に進むので、同時に 8 件を超えて待つ使い方は想定しない。
  ノード数の上限は async-opcua の `max_nodes_per_write` に渡し、超えた要求は
  サービス全体の `BadTooManyOperations` になる（Part 4 のとおり）。公開する
  Double は有限値に限るので、非有限値の Write はハンドラーを呼ばずに
  `BadOutOfRange` にする。書き込み可否は `RunningServer::set_tag_writable` で
  切り替え、登録したばかりのタグと再登録したタグは書き込み不可から始まる。
- 切断の扱い（2026-10-17、#25 で確認）: async-opcua はサービスの呼び出しを
  `tokio::spawn` で走らせ、要求の時間切れでだけ中止する。接続が切れても
  サービスは捨てられない。そこで、ノードごとにセッションがまだ有効かを確かめ、
  CloseSession の後は残りのノードを始めない（`BadSessionClosed`）。時間切れで
  中止されたときは、残りのノードは始まらない。CloseSession を送らずに TCP が
  切れたときは、async-opcua からそれを知る手段がなく、残りのノードも書く。
  書かれるのは 1 要求あたり `max_nodes_per_write` 件までで、同時実行数の上限も
  かかる。これも防ぐには、fork で接続の終了時にサービスのタスクを中止する
  必要がある。
- 切断の扱いの修正（2026-10-17、#25）: fork（[tyaro/async-opcua#2](https://github.com/tyaro/async-opcua/pull/2)）で、
  接続が終わったときに走っているサービスのタスクを中止するようにし、固定をそこへ
  移した（ADR-0001）。CloseSession を送らずに TCP だけが切れても、走っている
  1 件は独立したタスクなので最後まで終わり、まだ始めていない残りのノードは
  書かない。上のセッションの確認は、CloseSession を送ったときのために残す。
  中止は要求の時間切れと同じ経路で、すべてのサービスに及ぶ。キャッシュと
  node manager を順に更新する 2 段階のサービス（CreateMonitoredItems など）は、
  途中で中止されると中間状態が残りうる。本リポジトリの node manager は監視項目の
  状態を持たないので影響しないが、外部の購読状態を持つ node manager を書くときの
  前提になる。
- 試作の結果（2026-10-17、#25）: `SimpleNodeManager` を自前の実装に置き換えても、
  読み取り・購読の既存テストはすべて通った。async の Write で書き込み先の応答を
  待つ間も Read は待たされず、ユーザートークンから匿名かどうかを取れた。fork の
  修正は要らない。`InMemoryNodeManagerImpl` は `async_trait` で定義されているので、
  `async-trait` を直接の依存に足した（async-opcua の依存木に既にあり、許可する
  ライセンスは変わらない）。名前空間の登録は `SimpleNodeManager` の builder と同じく
  自前の builder で行う。

## 検討した代替案

- **案A（採用）: 利用者のハンドラーに通し、ノード管理を自前で実装する。**
  Hub の既存の安全策と監査をそのまま使える。ライブラリは Hub に依存しない。
  自前で持つのは書き込みと初期化だけで、読み取りと購読は async-opcua の既定実装
  のまま使える。
- **案B（不採用）: `SimpleNodeManager` の書き込みコールバックを使う。** 変更は
  小さいが、同期関数でロックを持ったまま呼ばれるので、PLC の応答を待てない。
  主体も受け取れず、利用者の監査に「誰が」を渡せない。
- **案C（不採用）: Hub が async-opcua の型を直接扱って書き込みを実装する。**
  ADR-0001 のクレート境界（MPL-2.0 の型を Hub の公開 API に載せない）に反する。
- **案D（不採用）: Phase 3 で認証を入れるまで書き込みを見送る。** 匿名で書ける
  ことがオーナーの要件で、読み取り専用が既定では使いにくい。認証は Phase 3 で
  主体を足す形で後から重ねられる。
- **案E（不採用）: サーバー側に書き込み専用の有効スイッチを置く。** REST や
  gRPC には口ごとのスイッチが無い。全体の停止は Hub の書き込みスイッチで行える。
- **案F（不採用）: サーバー側で待つ上限を設け、超えたら `BadTimeout` を返す。**
  応答は早く返せるが、書き込みが続いたまま再送を促し、同じ書き込みが二重に
  届く。重複を除く仕組み（操作 ID）を足すほどの利点がない。

## 帰結

- 匿名で書けるので、ループバック以外へ公開すると、同じネットワークの誰でも
  平文で PLC へ書ける。既定の待受がループバックであることが、実質的な防御に
  なる。利用者（Hub）は公開時の警告を管理画面にも出す（banto-industrial #653）。
- 「誰が書いたか」の記録は利用者の監査に任せる。サーバーは主体と接続元の情報を
  ハンドラーへ渡すだけで、自分では記録しない。匿名ではクライアントを
  ApplicationUri とセッションでしか区別できない。
- 回数制限の単位と、制限を超えたときの止め方・戻し方は利用者が決める。匿名の
  書き込みを 1 つの主体として数えると、1 つのクライアントの連打で OPC UA からの
  書き込みが全体で止まりうる（banto-industrial #653 で決める）。
- 公開する数値はすべて Double なので、整数のタグへの書き込みは利用者がタグの
  型へ戻す。Hub では、整数のタグへの小数は範囲外として拒否される。
- 複数ノードの Write の途中でクライアントが切断すると、書いたのは切断までに
  始めた分だけになり、クライアントはその結果を受け取れない。利用者の監査には、
  実際に書いた件数だけが残る。
- 匿名のクライアントの連打は、サーバーの同時実行数の上限と、利用者の回数制限の
  両方で抑える。
- 同時実行数の上限による `BadResourceUnavailable` はノード単位なので、複数ノードの
  Write は一部だけ書かれることがある。弾かれたノードは、クライアントが後から
  再送してよい。
- Write の応答は PLC の応答を待つ分だけ遅れる。クライアントが自分の時間切れで
  あきらめて再送すると、同じ値が二度書かれうる。これは REST・gRPC からの
  書き込みでも同じで、利用者の監査には両方が残る。
- 自前のノード管理を async-opcua の更新に追従させる。公開版への復帰（#17）の
  ときも、読み取り・購読・書き込みのテストで確かめる。
- Phase 3 で UserName と証明書の主体を足すとき、ハンドラーの契約は変えずに
  主体の列挙を広げる。UserName を平文で送らせないことも Phase 3 で決める。
- 書き込みの経路を足したので、相互接続テスト（Python asyncua）に Write を
  加える。asyncua は SourceTimestamp を付けて送るので、時刻を捨てる上の規則で
  そのまま通る。UaExpert の手動確認にも書き込みを加える。
