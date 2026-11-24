# ADR-0007: 数値と文字列のタグを元の型のまま公開する

- 状態: Accepted
- 日付: 2026-11-24
- 関連: [#56](https://github.com/tyaro/banto-opcua/issues/56)、README の「決定事項」2（2026-10-15）、[ADR-0002](0002-dynamic-reading-boundary.md)、[ADR-0004](0004-write-through-host-handler.md)、[ADR-0005](0005-client-as-hub-driver.md)、[banto-industrial #1324](https://github.com/tyaro/banto-industrial/issues/1324)

## コンテキスト

2026-10-15 の決定では、公開する DataType を Boolean と Double の 2 つにした。
ハブの現在値はスケーリング後の工学値で、宣言型の整数へ戻すと小数を失うためだった。
`string` は周期収集の対象外として公開しなかった。

記録計 ChronoGazer がハブの OPC UA サーバーを購読して記録する形になり
（banto-industrial #1324）、次の 2 点が困るようになった。

- 64 ビットの整数（積算のカウンタなど）は、2^53 を超えると Double で精度が落ちる。
- 元の型がわからないので、記録の側で整数か小数か（集計・表示の桁）を決められない。

## 決定

- core の `DataType` と `MappedValue` に Int16・UInt16・Int32・UInt32・Int64・UInt64・
  Float・String を足す。値の型と DataType は 1 対 1 で、サーバーは Variant と
  DataType 属性をその型のまま作る。
- 公開する型は `publish_data_type(TagType, ValueScaling)` で決める。スケーリング
  していない数値（`Raw`）は元の型、スケーリング後の工学値（`Scaled`）は従来どおり
  Double。`bit` は Boolean、`string` は String。スケーリングの有無はハブが知って
  いるので、ハブが渡す。`PublishDecision` と `publish_decision` は廃止する。
- 値の写像は丸めない（2026-10-15 の「品質と値」の契約を広げる）。
  - ハブの f64 からは `MappedValue::from_number` で変える。整数の型は、その型の範囲の
    整数だけを受ける。Float は f32 の範囲の有限値、Double は有限値を受ける。それ以外は
    値なし・汎用 Bad。String は数値から作らない。
  - 型の付いた値は `map_typed_value` で写す。型が DataType と違う値、非有限の
    Float / Double、上限を超える String は値なし・汎用 Bad。
  - 品質（Bad は値なし、Stale は `UncertainLastUsableValue` で最後の値を保持、
    Stale でも値が無効なら Bad）と時刻は、従来の契約のまま全部の型に当てはめる。
- String の上限は UTF-8 で 4096 バイト（`MAX_STRING_BYTES`）。超えた値は切り詰めない。
  切り詰めた文字列は元の値と区別できず、記録に誤った値が残るため。
- サーバーの `TagReading` は型の付いた値を持つ。`TagReading::new` は従来どおり
  `Option<f64>` を受けて `from_number` で変え、`TagReading::typed` は `MappedValue` を受ける。
- Write の検査（ADR-0004）も同じ型の組に広げる。Variant の型がノードの DataType と
  違えば `BadTypeMismatch`。非有限の Float / Double と上限を超える String は
  `BadOutOfRange`。サーバーは型を変換しない。
- クライアント（ADR-0005）の `ScalarValue` は、すでにこの型の組に SByte と Byte を
  足したものを扱う。`From<MappedValue> for ScalarValue` と
  `TryFrom<ScalarValue> for MappedValue` で行き来する。SByte と Byte は
  `NotPublishable` で断り、型を黙って広げない。

## 検討した代替案

- **案A（採用）**: 型の組を広げ、スケーリングの有無で Double に寄せるかを決める。
  記録の側が元の型を知ることができ、64 ビットの整数も精度を落とさない。
  スケーリング後の値は今までどおり Double なので、工学値の小数は失わない。
- **案B（不採用）**: 数値は全部元の型にする。スケーリング後の小数が整数の型へ
  入らず、Bad になるか、丸めることになる。2026-10-15 の決定の理由がそのまま残る。
- **案C（不採用）**: Double のまま、元の型をノードの属性（Description など）で
  知らせる。64 ビットの整数の精度の問題が解けず、クライアントが属性を読み分ける
  手間が増える。
- **案D（不採用）**: 範囲外や小数の値を、型の範囲へ切り詰める・丸める。
  記録に誤った値が残り、品質も Good のままになる。

## 帰結

- 公開 API が変わる（`DataType` と `MappedValue` の網羅的な `match`、
  `MappedValue` と `MappedReading` と `TagReading` の `Copy`、`TagReading::value` の型、
  `publish_decision` の廃止）。CHANGELOG に移行を書く。
- ハブは、タグごとにスケーリングの有無を `publish_data_type` に渡す。64 ビットの整数を
  精度を落とさずに公開するときは、f64 を経由せず `TagReading::typed` で渡す。
- タグの型を変えるときは、従来どおり削除して登録し直す（ADR-0002）。
- 型を足すときは、core の写像、サーバーの Variant と DataTypeId と Write の検査、
  クライアントとの変換を同じ組に保つ。
