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

基盤の async-opcua には次の制約がある（固定中の `60c4f9d` で確認）。

- `SimpleNodeManager::add_write_callback` のコールバックは同期関数で、
  アドレス空間の書き込みロックを持ったまま呼ばれる。書いた主体（セッションの
  ユーザートークン）も受け取らない。PLC の応答を待つ書き込みには使えない。
- `InMemoryNodeManagerImpl` を自前で実装すると、`write` は async になり、
  `RequestContext` からユーザートークンを取れる。
- SecurityPolicy `None` のエンドポイントでは、UserName トークンのパスワードは
  既定で平文になる。暗号化には RSA の秘密鍵操作が要り、ADR-0001 で無視している
  RUSTSEC-2023-0071 に触れる。

## 決定

サーバーは、利用者が渡す書き込みハンドラーを通してだけ Write を受け付ける。
不変条件は [conventions.md §2](../conventions.md#2-書き込みは利用者の書き込み経路を通す)。

### ハンドラーと主体

- ハンドラーは async のトレイトで、型は core の語彙で定義する。async-opcua の型は
  公開 API に出さない（ADR-0001、ADR-0002 と同じ境界）。
- ハンドラーに渡すもの: 対象タグの外部名（接続・グループ・タグ）、値
  （`Boolean` または `Double`）、書いた主体、接続元の情報（エンドポイントと
  セッション）。
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
- 書き込む DataValue に StatusCode や時刻が入っていたら
  `BadWriteNotSupported`、IndexRange 付きも `BadWriteNotSupported` にする。
- Write ではノードの値を書き換えない。PLC へ届いた値は、いつもどおり
  `update_tag_reading` で戻ってくる。確定していない値を Read や購読に見せない。

### 実行

- 書き込みは同期にする。ハンドラーの結果を待ってから Write に応答する。
  アドレス空間のロックを持ったまま待たない。
- 待つ上限をサーバーの設定に置き、超えたら `BadTimeout` にする。上限を超えた
  あとも、ハンドラーの処理は取り消さない（PLC へ届いたかは分からないため、
  利用者の監査に任せる）。
- 1 回の Write に複数のノードがあれば、要求の順に 1 件ずつハンドラーを呼び、
  結果をノードごとに返す。全件をまとめて成功・失敗にはしない（OPC UA の Write
  の意味どおり）。
- ハンドラーがあり、ループバック以外で待ち受けるときは、匿名の書き込みが
  外から届く。起動時に警告ログを出す。

### 拒否の理由と StatusCode

ハンドラーは成功か、core で定義する拒否の理由を返す。サーバーは次の表で
StatusCode に写す。

| 拒否の理由 | StatusCode | Hub の例 |
| --- | --- | --- |
| タグが無い | `BadNodeIdUnknown` | `not_found` |
| 書けないタグ | `BadNotWritable` | `not_writable` |
| 主体に権限が無い | `BadUserAccessDenied` | （Phase 3 の主体） |
| 型が合わない | `BadTypeMismatch` | `unsupported_value_type` |
| 値が範囲外 | `BadOutOfRange` | `value_out_of_range` |
| 書き込みを受け付けていない | `BadOutOfService` | `writes_disabled`、`collection_not_running`、`tag_disabled` |
| 回数制限 | `BadTooManyOperations` | `rate_limited` |
| 機器との通信に失敗 | `BadCommunicationError` | `write_failed` |
| 内部エラー | `BadInternalError` | `audit_write_failed`、`internal` |

### 実装方法

- 書き込みを受けるため、`InMemoryNodeManagerImpl` を自前で実装する。読み取り・
  購読の動作は今の `SimpleNodeManager` と変えない。
- 先に小さな試作で、読み取り・購読の既存テストを保ったまま async の Write と
  ユーザートークンを扱えるかを確かめる。だめなら、fork（tyaro/async-opcua）で
  コールバックに `RequestContext` を渡す修正を検討し、この ADR に追記する。

## 検討した代替案

- **案A（採用）: 利用者のハンドラーに通し、ノード管理を自前で実装する。**
  Hub の既存の安全策と監査をそのまま使える。ライブラリは Hub に依存しない。
  async-opcua の `SimpleNodeManager` の読み取りと購読の動作を自前で保つ手間が
  増える。
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

## 帰結

- 匿名で書けるので、ループバック以外へ公開すると、同じネットワークの誰でも
  平文で PLC へ書ける。既定の待受がループバックであることが、実質的な防御に
  なる。利用者（Hub）は公開時の警告を管理画面にも出す（banto-industrial #653）。
- 「誰が書いたか」の記録は利用者の監査に任せる。サーバーは主体と接続元を
  ハンドラーへ渡すだけで、自分では記録しない。
- 公開する数値はすべて Double なので、整数のタグへの書き込みは利用者がタグの
  型へ戻す。小数を整数のタグへ書いたときの扱いも利用者が決める。
- PLC の応答待ちで Write の応答が遅れる。待つ上限は、一般的なクライアントの
  要求の時間切れより短くする。
- 自前のノード管理を async-opcua の更新に追従させる。公開版への復帰（#17）の
  ときも、読み取り・購読・書き込みのテストで確かめる。
- Phase 3 で UserName と証明書の主体を足すとき、ハンドラーの契約は変えずに
  主体の列挙を広げる。UserName を平文で送らせないことも Phase 3 で決める。
- 書き込みの経路を足したので、相互接続テスト（Python asyncua）に Write を
  加える。UaExpert の手動確認にも書き込みを加える。
