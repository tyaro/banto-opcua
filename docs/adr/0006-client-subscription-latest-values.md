# ADR-0006: クライアントの購読モードでは、購読で受けた最新値を部品が持つ

- 状態: Proposed
- 日付: 2026-10-18
- 関連: [ADR-0005](0005-client-as-hub-driver.md)、[#33](https://github.com/tyaro/banto-opcua/issues/33)、[banto-industrial #678](https://github.com/tyaro/banto-industrial/issues/678)

## コンテキスト

ADR-0005 では、クライアントの部品は周期 Read で収集し、購読（Subscription）は
「broker の周期モデルとの関係を決めてから別の ADR で足す」とした。2026-10-18 に
オーナーが購読モードも実装すると決め、次の 2 点を選んだ。

1. 購読で受けた値は、部品がノードごとの最新値として持つ。Hub のドライバは周期ごとに
   その最新値を取り出す。broker の周期モデルは変えない。周期の間に何度変わっても、
   渡すのは最後の値になる。
2. 周期 Read か購読かは接続ごとに選ぶ。1 つの接続の中のタグは、全部同じ読み方にする。

購読を使うと、変化があったときだけ値が届くので、周期 Read より相手のサーバーの負荷が
小さく、周期より短い変化の時刻（SourceTimestamp）も最後の 1 回分は正しく残る。

async-opcua 0.19（fork `9ee772b`）のクライアントの購読について、実装に効く事実を
ソースで確かめた。

- 通知のコールバックは、イベントループのタスクの中で、購読の状態のロックを持ったまま
  呼ばれる。待つ処理や `Session` のメソッドを呼ぶと止まる。
- MonitoredItem は `client_handle` で見分けられる（0 以外なら指定した値のまま使われる）。
  作った直後の現在値は、サーバーが最初の通知として送る（Part 4 の Reporting）。
- 自動再接続の既定（`recreate_subscriptions = true`）では、張り直した後に購読を作り直すが、
  **古い NodeId（名前空間の番号）をそのまま使う**。None / Anonymous の接続では
  TransferSubscriptions がサーバーに拒否されるので、毎回作り直しになる。
- 作り直した後、Publish の要求が再開しないことがある（作り直しの中で出す
  `trigger_publish_now` が、後から作られるイベントループの状態に届かない）。
- Publish の応答が失われても Republish はせず、通知の番号の抜けも見ない。最後の変化を
  落としたノードは、次に変わるまで古い値のままになる。
- `create_monitored_items` は要求を分けない。async-opcua のサーバーは 1 回の要求に
  1000 ノード（`MaxMonitoredItemsPerCall`、`i=11714`）、1 つの購読に 1000 ノードまで。
- サーバーが購読を捨てたとき（lifetime の時間切れ）は `StatusChangeNotification` が
  届くが、クライアントの購読の状態からは消えない。
- 通知の StatusCode には、キューのあふれのビット（0x80）が立つことがある。

## 決定

### 設定と API

- `ClientConfig` に読み方を足す。既定は周期 Read（今までどおり）。
  - 購読を選ぶと、Publish の間隔、sampling の間隔、購読の lifetime と keep-alive の回数を
    受け取る。キューの大きさは 1、あふれたら古い方を捨てる。データ変化のフィルタは
    付けない（サーバーの既定の StatusValue。値か StatusCode が変わったら通知）。デッド
    バンドは後回し。
- 購読モードの接続では、Hub が収集するノードの並びを登録し、最新値を取り出す。
  - 登録: ノードの並びを受け取り、購読を作り直して MonitoredItem を作る。ノードごとの
    作成の StatusCode を返す。並びを変えるときは、登録し直す（差分の更新は後回し）。
  - 取り出し: 登録した順に、ノードごとの最新の値・StatusCode・SourceTimestamp
    （[`NodeReading`]）を返す。`read_batch` と同じ形で、Hub のドライバは
    `BrokerSession::read_batch` にそのまま写せる。
  - まだ最初の通知が届いていないノードは、値なし・`BadWaitingForInitialData`。作成に
    失敗したノード（`BadNodeIdUnknown`、解決できない `nsu=` など）は、その StatusCode。
  - 取り出しの `Err` は、`read_batch` と同じく接続ごとの失敗。
- 購読モードでも `write_batch` はそのまま使える。`read_batch` も使える（周期 Read の
  代わりではなく、必要なときの読み直しとして）。

### 最新値の持ち方

- 最新値は、登録した並びの位置で引ける表に持つ。`client_handle` は「位置 + 1」にする。
- コールバックは、表を短いロックで書き換えるだけにする（待たない、`Session` を呼ばない）。
- 表の各行には、どの接続の世代と登録で作ったかを持たせる。古い世代・古い登録の
  通知は捨てる（削除の応答が届く前にも通知は来る）。
- 表の各行は、MonitoredItem の作成に成功したかを持つ。失敗した行は、登録し直して
  成功するまで作成の StatusCode のままにする（通知が来ないので、ほかの経路で値を
  入れると、その値のまま固まる）。
- 表の各行は、通知を受けるたびに増える数（更新の番号）を持つ。
- StatusCode は受けたまま持つ。あふれのビットなど情報のビットが立つことがあるので、
  Hub は品質を深刻度（Good / Uncertain / Bad）で決め、`== Good` では比べない。

### 取りこぼしと購読の喪失

- Publish が失敗したとき（`PublishFailed`）は、通知を落としたかもしれない。次の
  取り出しの前に、MonitoredItem の作成に成功した行のノードを Read でまとめて読み直し、
  表へ入れてから返す（読み直しは `read_batch` と同じ分け方）。
  - 作成に失敗した行は読み直さない（上の「作成に成功したか」）。
  - 読み直しの間も表のロックは持たない。Read を送る前に各行の更新の番号を覚え、結果を
    入れるときに番号が変わっていない行だけを書き換える。読み直しの間に届いた通知の
    方が新しいので、古い Read の結果で戻さない（分けて送った Read の後の塊を待つ間に、
    前の塊のノードへ通知が届くことがある）。
- 次のときは、購読を失ったとみなす。購読を作り直すまで、取り出しを接続ごとの失敗にする。
  - サーバーが購読を捨てたと知らせた（`StatusChangeNotification` が Bad）。
  - Publish が `BadNoSubscription` か `BadSubscriptionIdInvalid` で失敗した。
    StatusChangeNotification を取りこぼしても、この経路で分かる。async-opcua は
    `BadNoSubscription` を受けると定期の Publish を止める（`event_loop_state.rs`）ので、
    Read で読み直すだけでは購読は戻らず、そのときの値のまま固まる。

  作り直すのは次のとおり。
  - 自動再接続が無い（既定）なら、Hub の broker が張り直す（新しい接続で登録し直す）。
  - 自動再接続なら、部品が購読を作り直す（下の「再接続」の 1・3・4 と同じ手順）。

### 再接続

- async-opcua の購読の作り直し（`recreate_subscriptions`）は止める。古い名前空間の番号で
  作り直すため、名前空間が変わったサーバーで別のノードを監視しうる（ADR-0005 の Write と
  同じ問題）。
- 自動再接続が無い（既定）とき: 接続が切れたら、取り出しは `Error::Disconnected`。
  張り直すのは broker で、新しい接続で登録し直す。
- 自動再接続（`ReconnectPolicy::Automatic`）のとき: 新しい世代になったら、部品が
  1. 古い購読を消す（クライアントの状態を消すため。サーバーには無いので Bad でよい）、
  2. NamespaceArray と OperationLimits を読み直す（ADR-0005 と同じ）、
  3. 登録した並びから購読を作り直し、表を「最初の通知待ち」に戻す、
  4. Publish を始めさせる（`trigger_publish_now`。async-opcua の再開の漏れへの対処）。
  作り直すまでの取り出しは `Error::Disconnected`。

### 要求の分け方

- MonitoredItem の作成は、接続時に読む `MaxMonitoredItemsPerCall`（`i=11714`）ずつ
  分けて送る。広告が無いときは設定の既定値。`BadTooManyOperations` では半分にして
  送り直す（ADR-0005 の Read / Write と同じ）。
- 1 つの購読に入れるノード数の上限（`MaxMonitoredItemsPerSubscription`）を超えたら、
  `BadTooManyMonitoredItems` を作成の StatusCode として返す。購読を複数に分けるのは
  後回し。

## 検討した代替案

- **案A（採用）: 最新値を部品が持ち、周期ごとに取り出す。** broker の周期モデルを
  変えずに済み、Hub のドライバは Read と同じ形で書ける。周期の間の変化は最後の 1 回に
  まとまる。
- **案B（不採用）: 変化をそのまま Hub へ流す。** 周期の間の変化も全部届くが、broker を
  イベント駆動に変える必要があり、Hub 側の改修が大きい。必要になったら、表と並べて
  ストリームを足せる形にしておく。
- **案C（不採用）: タグごとに Read と購読を選ぶ。** 速く変わるタグだけ購読、などが
  できるが、部品も Hub の画面も複雑になる。接続を分ければ同じことができる。
- **案D（不採用）: async-opcua の購読の作り直しを使う。** 古い名前空間の番号で作り直す
  ので、名前空間が変わったサーバーで別のノードを監視しうる。

## 帰結

- Hub のドライバは、接続の設定で読み方を選び、購読モードでは登録と取り出しを使う
  （banto-industrial #678）。品質と時刻への写像は周期 Read と同じ。
- 周期の間の変化は最後の 1 回にまとまる。変化の履歴が必要になったら、案B のストリームを
  別の ADR で足す。
- Publish の失敗のたびに Read で読み直すので、通信が不安定なときは Read の負荷が増える。
- デッドバンド、購読の分割、登録の差分の更新は後回し。
- 購読モードのプロセス内テストは本リポジトリのサーバーを相手にし、相互接続テストは
  opc-plc の変化するノードを相手にする。

[`NodeReading`]: ../../crates/banto-opcua-client/src/convert.rs
