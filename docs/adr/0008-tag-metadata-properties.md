# ADR-0008: タグの説明・単位・レンジを、変数ノードの Description と標準のプロパティで公開する

- 状態: Accepted
- 日付: 2026-11-24
- 関連: [#55](https://github.com/tyaro/banto-opcua/issues/55)、[ADR-0002](0002-dynamic-reading-boundary.md)、[ADR-0007](0007-publish-native-data-types.md)、[banto-industrial #1324](https://github.com/tyaro/banto-industrial/issues/1324)、[banto-industrial #678](https://github.com/tyaro/banto-industrial/issues/678)

## コンテキスト

記録計 ChronoGazer がハブの OPC UA サーバーを購読して記録する（banto-industrial #1324）。
トレンドの軸、帳票の見出し、アプリが無くても読める控えのファイルには、タグの単位・説明・
レンジが要る。ハブはタグの単位を持っているが、v0.2.0 のサーバーの変数ノードには
Description・EngineeringUnits・EURange が無く、渡す口も無かった。

決めることは 3 つあった。ノードの型定義、属性が変わったときに同じノードのまま書き換えるか、
変わったことをクライアントにどう知らせるか。

## 決定

- **入力。** core の `TagMetadata`（説明、`EngineeringUnits`、`EuRange`、`TwoStateLabels`。
  どれも任意）を、`add_tag_reading_with_metadata` で登録時に、`update_tag_metadata` で後から渡す。
  単位とレンジは数値のタグだけ、二値の文言は Boolean のタグだけに付けられ、
  `TagMetadata::validate` が断る。文字列は ADR-0007 と同じ 4096 バイトまで。
- **写し方。** 説明は Description 属性にする。単位・レンジ・文言は、名前空間 0 の標準の
  BrowseName（`EngineeringUnits`・`EURange`・`TrueState`・`FalseState`）のプロパティにし、
  HasProperty で辿れるようにする。単位は EUInformation にする。UNECE の Common Code があれば
  UNECE の名前空間と OPC 10000-8 の算法の UnitId を入れ、無ければ UnitId を -1、
  NamespaceUri を null にして、表示の文字列だけを持たせる。
- **型定義。** 属性から選ぶ（`TagMetadata::shape`）。
  - EURange があれば AnalogItemType（EURange が必須の型）。
  - 単位だけなら BaseAnalogType（どちらも任意の型）。
  - 文言のある Boolean は TwoStateDiscreteType（TrueState・FalseState が必須の型）。
  - ほかは今までどおり BaseDataVariableType。
  UaExpert など多くのクライアントは、型定義を見て単位やレンジを表示する。
  BaseDataVariableType に属性を足すより読み取りやすい。
- **変更。** 型定義と、どのプロパティを持つか（ノードの形）が同じなら、同じ NodeId のまま
  Description とプロパティの値を書き換え、購読者へ通知する。
  - 単位・レンジ・文言の値が変わったときは、タグ本体の Value の購読者にも、StatusCode に
    SemanticsChanged のビットを立てた通知を送る（OPC 10000-8 5.2、BaseAnalogType・
    TwoStateDiscreteType の規定）。値・品質・時刻はそのままで、次の値の更新でビットは消える。
    説明だけの変更では立てない。
  - 意味を決めるプロパティの AccessLevel には、CurrentRead に SemanticChange を足す
    （OPC 10000-3 5.6.2）。書き換わりうるプロパティだとクライアントに知らせる。
  - 利用する側（クライアント）は、SemanticsChanged のビットを見たらプロパティを読み直す。
    プロパティの Value を購読していれば、新しい値も届く。
  - 形が変わる変更（レンジを付ける・外すなど）は書き換えずに `Error::MetadataShapeChanged` を
    返す。そのときに限り、利用する側はタグを削除して登録し直す。不正な属性（型に合わない、
    レンジが逆など）は `Error::TagUpdate` で、登録し直しても通らない。DataType の変更を削除・
    再登録で表す ADR-0002 と同じ扱いで、購読者には `BadNodeIdUnknown` から復帰する形で伝わる。
  - 登録し直したときは、タグ本体と、作り直したプロパティの購読者へ通知し、同じ監視項目のまま
    Good に戻す。登録し直さなかったプロパティの購読は `BadNodeIdUnknown` のまま残る。
- **ModelChangeEvent は出さない。** async-opcua の `GeneralModelChangeEventType` は変更を
  配列でなく 1 件で持ち、NodeVersion の仕組みも無い。対応するクライアントも少ない。
  形の変化は上の削除・再登録で表し、値の変化は購読で伝える。
- **プロパティの NodeId。** タグの NodeId（文字列）とも、フォルダーの ByteString とも重ならない
  ByteString にする。タグを削除するときはプロパティも消し、購読者へ `BadNodeIdUnknown` を通知する。

- **クライアント（ADR-0005）。** `ConnectedClient::read_metadata` が、ノードの並びの Description と、
  同じ BrowseName のプロパティ（HasProperty とその下位の参照を TranslateBrowsePathsToNodeIds で
  辿る）を読み、`TagMetadata` で返す。型定義は問わないので、他社のサーバーの単位も取り込める
  （banto-industrial #678）。UNECE の名前空間の UnitId はコードに戻す。無い・読めない・型の違う
  プロパティは `None` にし、ノード全体を失敗にはしない。Description は任意の属性（OPC 10000-3 5.2）
  なので、`BadAttributeIdInvalid` は説明が無いだけとしてプロパティを読む。プロパティを辿らないのは
  ノードが無いとき（`BadNodeIdUnknown`・`BadNodeIdInvalid`）だけ。

## 検討した代替案

- **案A（採用）**: 属性に応じて標準の型定義を選び、形の変化は削除・再登録にする。
  標準どおりなので他社のクライアントでも読める。変更の扱いが ADR-0002 と揃う。
- **案B（不採用）**: 型定義は BaseDataVariableType のまま、プロパティだけを足す。
  簡単だが、型定義を見て単位を表示するクライアントで出ない。
- **案C（不採用）**: 形が変わる変更も、同じ NodeId のまま、プロパティを足し引きして行う。
  クライアントはブラウズし直さないと気づけず、ModelChangeEvent が使えない以上、
  変化が黙って起きる。
- **案D（不採用）**: 数値のタグには、属性が無くても常に EngineeringUnits と EURange を置く。
  形は変わらないが、無いレンジを表す正しい値が Range に無い。

## 帰結

- ハブは、タグの単位などが変わったら、まず `update_tag_metadata` を呼ぶ。
  `Error::MetadataShapeChanged` のときだけ、`remove_tag` と `add_tag_reading_with_metadata` で
  登録し直す（書き込み可の設定もやり直す）。
- 記録する側は、SemanticsChanged のビットを見たらプロパティを読み直す（プロパティの Value を
  購読していればそれでもよい）。接続のたびにも読み直す。
- プロパティを足すときは、core の `MetadataShape` と、サーバーのプロパティの一覧・NodeId・
  削除の処理を同じ組に保つ。
