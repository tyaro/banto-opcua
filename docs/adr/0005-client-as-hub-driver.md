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
  要求の時間切れを受け取る。
- **自動再接続を止める:** async-opcua の自動再接続は止め、接続が切れたら、それ以降の
  呼び出しは接続ごとの失敗を返す。張り直すのは Hub の broker で、新しく接続する。
- **まとめて Read:** NodeId の並びを受け取り、ノードごとに次を返す。
  - 値（下の型。値が無いときは `None`）
  - StatusCode
  - SourceTimestamp
- **まとめて Write:** NodeId と値の並びを受け取り、ノードごとの StatusCode を返す。
- **要求の分割:** 接続時にサーバーの `ServerCapabilities/OperationLimits` の
  `MaxNodesPerRead` / `MaxNodesPerWrite` を読み、超える要求は分けて送る。値が無ければ
  設定の既定値を使う。
- **失敗の分け方:** 接続ごとの失敗（切断、時間切れ、サービス全体のエラー）は `Err`、
  ノード単位の失敗は結果の StatusCode にする。`BrokerSession` の約束にそのまま写せる
  形にするため。

### core に置くもの

- **NodeId の文字列表現の解析:**
  - 形は `ns=<index>;i=<number>`、`ns=<index>;s=<string>`、`nsu=<uri>;i=…`、
    `nsu=<uri>;s=…`。`ns=` を省いた形は名前空間 0。
  - GUID（`g=`）と ByteString（`b=`）は、当面は解析エラーにする。
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
- **案C（不採用）: 部品が再接続も受け持つ（async-opcua の自動再接続を使う）。** broker の
  再接続と二重になり、どちらが張り直しているのか分かりにくい。切断の間の品質（Bad）の判断も
  broker から見えなくなる。
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
