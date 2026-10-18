# ADR-0005: OPC UA クライアントを Hub のドライバ向けの部品として作る

- 状態: Accepted
- 日付: 2026-10-17
- 関連: [ADR-0001](0001-async-opcua-base.md)、[ADR-0004](0004-write-through-host-handler.md)、[#33](https://github.com/tyaro/banto-opcua/issues/33)、[banto-industrial #678](https://github.com/tyaro/banto-industrial/issues/678)、[banto-industrial #201](https://github.com/tyaro/banto-industrial/issues/201)

## コンテキスト

これまで `banto-opcua-client` は足場で、サーバーのプロセス内テストと相互接続テストが
使う接続と `read_u32` だけを持っていた。製品としてのクライアントは banto-industrial
#201 の対象外として残していた。

2026-10-17 のオーナー決定で、OPC UA クライアントは SLMP・Modbus と同列の
ドライバとして Hub に置くことになった。外部の OPC UA サーバー（PLC の OPC UA
ユニット、ゲートウェイなど）のタグを、ほかのドライバと同じように収集・書き込みする。
同じ日に次も決まった。

1. 最初は SecurityPolicy None / Anonymous だけにする。署名・暗号化と UserName は、
   サーバー側の Phase 3 と一緒に入れる。
2. 読み方は周期 Read にする。購読（Subscription）は後から足す。
3. 書き込みは最初から入れる。
4. タグは NodeId を手で入力する。相手サーバーの Browse での選択は後回しにする。

Hub のドライバは `banto-broker` の `BrokerSession`（`read_batch` / `write_batch` /
`disconnect`）と、接続を張る `Connector` で表す。周期収集と再接続は broker の
状態機械が受け持ち、ドライバは接続を張ってまとめて読み書きするだけにする。
`Err` は接続ごとの失敗で、broker はそれを見て接続を張り直す。タグ単位の失敗は
結果の `Bad` にする。SLMP と Modbus のプロトコルの部品（`banto-plc`）も、再接続の
ループは持たない。

async-opcua のクライアントは、既定でセッションの再接続を自分で繰り返す
（`session_retry_limit` など）。

## 決定

プロトコルの部品は `banto-opcua-client` に置き、Hub のドライバ（`BrokerSession` の
実装）は banto-industrial に置く。部品の公開 API は core の語彙だけで定義し、
async-opcua の型は出さない（ADR-0001、ADR-0002 と同じ境界）。

### 部品が持つもの

- **接続:** SecurityPolicy None / Anonymous で、エンドポイント URL・接続の時間切れ・
  要求の時間切れ・keep-alive の間隔と失敗回数を受け取る。async-opcua の keep-alive は
  既定で切断を検知しない（`max_failed_keep_alive_count` が 0）ので、静かな切断を
  見つけられるよう必ず設定する。要求の時間切れは収集の周期より短くし、keep-alive の
  間隔はセッションの時間切れより短くする（長いとセッションが失効する）。
- **自動再接続を止める:** 接続が切れたら、それ以降の呼び出しは接続ごとの失敗を返す。
  張り直すのは Hub の broker で、新しく接続する。async-opcua では
  `session_retry_limit(0)` だけでは切断後に 1 回張り直しを試すので、
  `Session::disable_reconnects()` も呼ぶ。接続の前に呼んでも効くので、セッションを
  作った直後、イベントループを動かす前に呼ぶ（初回の接続と切断の間の競合が無い）。
- **部品の側で張り直す選択肢（2026-10-18 追記、オーナーの指示）:** banto の broker の
  仕様が変わったときのために、async-opcua の自動再接続で部品が張り直す設定も残す
  （`ReconnectPolicy::Automatic`）。既定は上の「止める」のまま。
  - 張り直している間の呼び出しは、接続ごとの失敗を返す。
  - 張り直した後は、最初の要求の前に NamespaceArray と OperationLimits を読み直す
    （サーバーの再起動で番号が変わることがある）。
  - 張り直したことはイベントループの `Reconnected` で知る。そのため部品は
    `SessionEventLoop::spawn` ではなく `enter` のストリームを自分のタスクで回す。
  - この設定を使うときは、Hub の broker が同じ接続を張り直さないようにする（二重に
    なる。案C の懸念）。
- **まとめて Read:** NodeId の並びを受け取り、ノードごとに次を返す。
  - 値（下の型。値が無いときは `None`）
  - StatusCode（そのまま返す。Good / Uncertain / Bad をどの品質に写すかは Hub が決める）
  - SourceTimestamp（サーバーが付けなければ `None`。現在時刻で補わない）

  `TimestampsToReturn` は `Source`、機器から読み直すよう `maxAge` は 0 にする。
- **まとめて Write:** NodeId と値の並びを受け取り、ノードごとの StatusCode を返す。
  送るのは Value 属性だけで、IndexRange は付けない。DataValue には値だけを入れ、
  StatusCode も時刻も付けない（時刻付きの Write を拒否するサーバーがある）。
- **空の並び:** 送らずに空の結果を返す。async-opcua は空の要求を `BadNothingToDo` の
  エラーにするので、そのまま返すとタグが 0 件の接続で broker が再接続を繰り返す。
- **要求の分割:** 接続時に `ServerCapabilities/OperationLimits` の `MaxNodesPerRead`
  （`i=11705`）と `MaxNodesPerWrite`（`i=11707`）を読み、超える要求は分けて送る。
  - 値が 0（制限なし）、ノードが無い、ノードの StatusCode が Bad のときは、設定の
    既定値を使い、接続ごとの失敗にしない。Read のサービス自体が失敗した（切断、
    時間切れ）ときは、接続ごとの失敗にする。接続時の NamespaceArray の Read も同じ。
  - 広告値を守っても `BadTooManyOperations` を返すサーバーがある。サービス全体の拒否で
    塊は適用されていないので、拒否された塊とそれ以降を半分の大きさにして送り直す。
    1 ノードの塊でも拒否されたら、そこで接続ごとの失敗にする。
- **失敗の分け方:** 接続ごとの失敗（切断、時間切れ、サービス全体のエラー）は `Err`、
  ノード単位の失敗は結果の StatusCode にする。`BrokerSession` の約束にそのまま写せる
  形にするため。応答の結果の数が要求と合わないときも、接続ごとの失敗にする。
- **分けた Write の途中で失敗したとき:** Read は読み直せば済むが、Write は先に送った分が
  機器に届いている。そこで Write の `Err` には、ノードごとの結末を持たせる。
  - **書いた結果:** 応答を受け取り、結果の数が要求と合った分は、その StatusCode。
  - **書かれていない（確実に）:** 次の閉じた一覧に限る。
    - 送る前に失敗した分（`BadNotConnected`）と、まだ送っていない分。
    - 応答のサービス全体の結果が Bad だった分（`BadTooManyOperations`、
      `BadSessionIdInvalid` など。適用されていないことが確定している）。拒否の
      StatusCode を持たせる。
  - **結果不明（届いたかもしれない）:** 要求を送った後の、上に当たらない失敗すべて。
    たとえば `BadTimeout`・`BadConnectionClosed`・`BadCommunicationError`、応答の
    符号化の上限超過（`BadEncodingLimitsExceeded`。サーバーは適用済みのことがある）、
    応答の結果の数が要求と合わないとき。失敗の StatusCode を持たせる。

  分からないものを「書かれていない」に入れると、適用されたかもしれない Write を未適用と
  監査に残してしまう。そこで「書かれていない」を閉じた一覧にし、残りを結果不明にする。

  Hub のドライバはこれを見て、書けたノード・書けたか分からないノード・書いていない
  ノードを区別して返し、監査に残す（banto-industrial #678）。

  実装（2026-10-18、#33）: `Session::write` はサービスの拒否も送信の失敗も同じ
  `Error`（StatusCode）にして返すので、「要求ごとの拒否」と「送った後の失敗」を分けられ
  ない。そこで Write の要求を自分で組み立て（ヘッダーは `services::Write` の
  `header()` から取る）、`session.channel().send()` で送る。
  - `send` の `Err` のうち、送る前と分かるのは `BadNotConnected` だけ（転送路が無いときに
    async-opcua が返す）。ほかは結果不明にする。
  - 応答を受け取れたら、Write の応答のサービスの結果、または ServiceFault の結果が Bad の
    ものを拒否にする。想定外の応答と、結果の数の不一致は結果不明（`BadUnknownResponse`）。
  - 自動再接続のとき、塊を送る前に張り直していたらその塊から先は送らない。応答の後に
    張り直していたら、古い名前空間番号で新しい接続へ送った可能性があるので、その塊を
    結果不明（`BadConnectionClosed`）にする。
  - 公開 API では、結末を `WriteOutcome` の `Completed`（書いた結果）、`Unknown`
    （結果不明）、`Rejected` と `NotSent`（書かれていない）で表す。

### core に置くもの

- **NodeId の文字列表現の解析:**
  - 形は `ns=<index>;i=<number>`、`ns=<index>;s=<string>`、`nsu=<uri>;i=…`、
    `nsu=<uri>;s=…`。`ns=` を省いた形は名前空間 0。`s=` の識別子は、`;` を含んでも
    残りすべてを取る。
  - 解析の結果は、名前空間を番号か URI のどちらかで持つ新しい型（ExpandedNodeId に
    相当）にする。既存の `NodeId`（番号だけを持つ）はサーバー側の語彙として変えない。
    クライアントの部品が接続時に、この型を `NodeId` へ解決する。
  - `nsu=` の形は Part 6 の ExpandedNodeId の文字列表現で、URI の `;` は `%3B`、`%` は
    `%25` に符号化されている。解析でこの 2 つだけを大文字・小文字を問わず復号し、
    復号した URI を NamespaceArray と比べる。ほかの `%XX` は URI 自体の符号化
    （`urn:my%20ns` など）なので復号しない（2026-10-18、#33。async-opcua と同じ）。
    たとえば `nsu=urn:vendor:a%3Bb;s=Tag` の URI は `urn:vendor:a;b`。表示するときは
    `;` と `%` を符号化し直す。
  - `svr=` は 0（このサーバー）だけを受け付ける。async-opcua の ExpandedNodeId の表示は
    `svr=0;` を付けるため。0 以外（別のサーバー）は受け付けない。
  - 並びは Part 6 の順だけを受け付ける（python asyncua の表示のように識別子の後ろに
    `nsu=` を置く形は受け付けない）。前後の空白は解析で除かず、Hub が除く。
  - GUID（`g=`）と ByteString（`b=`）は、当面は「未対応の識別子の型」として、ほかの
    解析エラーと区別して返す。
  - 表示では `ns=0;` を省かない（2026-10-18、#33）。Part 6 の正規形は省く形だが、既存の
    `NodeId` の `Display` と同じ形にそろえる。解析は省いた形も受け付ける。
  - Hub は登録時にこの解析を使い、解析できないアドレスを登録させない。
- **値の型:** OPC UA のスカラーの組み込み型（Boolean、SByte〜Int64、Byte〜UInt64、
  Float、Double、String）に対応する列挙。
  - 配列とそれ以外の型は「対応しない値」としてノード単位の Bad にする。
  - サーバー側の `MappedValue`（Boolean と Double だけ）とは分ける。サーバーは公開する
    型を Double に固定しているが、クライアントは相手のノードの型をそのまま扱うため。

### 部品が持たないもの

- **型の変換:** Write では値の型を変換しない。Hub がタグの型から値の型を選び、合わなければ
  サーバーの `BadTypeMismatch` がそのまま返る。Read の値の変換と、品質・時刻への写像も
  Hub がする。
- **周期・再接続・品質の判断:** broker と Hub の収集エンジンの役目。
- **`nsu=` の解決以外の名前の扱い:** BrowsePath での指定や Browse は後回し。

### 名前空間 URI の解決

`ns=` の番号はサーバーの再起動や設定変更で変わることがあるので、`nsu=` も受け付ける。
部品は接続のたびに NamespaceArray を読み、`nsu=` を番号に直す。再接続は新しい接続なので、
そのたびに解決し直す。URI が NamespaceArray に無いノードは、ノード単位の
`BadNodeIdUnknown` にする（接続ごとの失敗にはしない）。

## 検討した代替案

- **案A（採用）: 部品は banto-opcua、ドライバは Hub に置く。** SLMP・Modbus と同じ分担で、
  async-opcua の型が Hub に漏れない。broker の再接続と周期の仕組みをそのまま使える。
- **案B（不採用）: Hub が async-opcua のクライアントを直接使う。** ADR-0001 のクレート境界
  （MPL-2.0 の型を Hub の公開 API に載せない）に反する。async-opcua の更新や fork の固定の
  影響が Hub に直接及ぶ。
- **案C（既定にはしない。設定として残す）: 部品が再接続も受け持つ（async-opcua の自動再接続を
  使う）。** broker の再接続と二重になり、どちらが張り直しているのか分かりにくい。切断の間の
  品質（Bad）の判断も broker から見えなくなる。2026-10-18 に、broker の仕様の変更に備えて
  設定で選べるようにした（上の「部品の側で張り直す選択肢」）。
- **案D（不採用）: 最初から購読で収集する。** 変化のあったときだけ届くので効率は良いが、
  broker の周期モデルと合わない。周期 Read で動かしてから、購読を別の作業で足す。
- **案E（不採用）: 部品が Write の値をノードの DataType へ変換する。** 書く前にノードの
  DataType を読む往復が増え、Hub のタグの型との二重管理になる。

## 帰結

- 平文・匿名でしか接続できないので、署名・暗号化を求める実機のサーバー（多くの PLC の
  OPC UA ユニット）には、Phase 3 まで接続できない。Hub は平文・匿名で外部へ接続している
  ことを管理画面に表示する。
- Phase 3 で暗号化を入れると、ADR-0001 で無視している RUSTSEC-2023-0071（RSA の秘密鍵の
  操作）にクライアントも当たる。そのときに見直す。
- 周期 Read なので、収集の周期ぶんの遅れと、変化が無くても読む負荷がある。購読は、broker
  の周期モデルとの関係を決めてから別の ADR で足す。
- 値の型は相手のノードのものをそのまま返すので、Hub のタグの型との変換と範囲の確認は、
  Hub のドライバが受け持つ。
- `nsu=` の解決のため、接続のたびに NamespaceArray を 1 回読む。
- クライアントの相互接続テストは opc-plc を相手に広げる。サーバーとクライアントを同じ
  リポジトリに置いているので、プロセス内テストは本リポジトリのサーバーを相手にできる。
