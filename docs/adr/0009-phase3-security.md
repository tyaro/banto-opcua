# ADR-0009: 暗号化したエンドポイントを既定にし、利用者の照合と証明書の信頼の一覧を利用する側へ渡す

- 状態: Accepted
- 日付: 2026-11-24
- 関連: [#54](https://github.com/tyaro/banto-opcua/issues/54)、[ADR-0001](0001-async-opcua-base.md)、[ADR-0003](0003-persistent-pki-dir.md)、[ADR-0004](0004-write-through-host-handler.md)、[ADR-0005](0005-client-as-hub-driver.md)、[banto-industrial #1324](https://github.com/tyaro/banto-industrial/issues/1324)、[banto-industrial #678](https://github.com/tyaro/banto-industrial/issues/678)

## コンテキスト

サーバーとクライアントは SecurityPolicy `None`・MessageSecurityMode `None`・Anonymous
だけで、サーバーはクライアントの証明書をすべて信頼していた（`trust_client_certs(true)`）。
クライアントはサーバーの証明書の検査を丸ごと止めていた（`verify_server_certs(false)`）。
banto-hub も OPC UA の公開を診断用とし、ループバックを既定にしていた。

2026-11-24 のオーナー決定で、記録計 ChronoGazer を Hub に OPC UA でつなぐことになった
（banto-industrial #1324）。両者は別の機械に置くことが多く、LAN 越しの本番の接続になる。
Hub の OPC UA クライアントのドライバ（banto-industrial #678）も、暗号化を求める他社の
サーバーにつなぐ。README のロードマップの Phase 3（証明書、SecurityPolicy、認証、
トラスト）をここで入れる。

前提になる async-opcua の振る舞い:

- サーバーはクライアントの証明書を CreateSession の要求で確かめ、クライアントは
  サーバーの証明書をその応答で確かめる。どちらも `pki_dir` の `rejected/` にあれば断り、
  `trusted/` にあれば有効期間などを確かめ、どちらにも無ければ `rejected/` に置いて断る。
  ディレクトリは接続のたびに読む。断った理由の StatusCode は、どちらの側が断っても同じ
  （`BadCertificateUntrusted` など）。
- サーバーの証明書の検査には、利用者が差し込む口が無い。利用者の照合は `AuthManager`
  トレイトで差し替えられる。
- 自己署名の証明書の SubjectAltName には、`localhost`・ループバックの IP・この機械の
  ホスト名とその IP が載る。公開ホスト（`advertised_host`）は載らない。

## 決定

### サーバー

- **エンドポイント:** 既定で Basic256Sha256・Aes128_Sha256_RsaOaep・Aes256_Sha256_RsaPss
  の各々を Sign と SignAndEncrypt で開く（6 つ）。`ServerConfig::with_security_policies` /
  `with_security_modes` で絞れる。廃止扱いの Basic128Rsa15・Basic256 は扱わない。
  `None` のエンドポイントは `with_none_endpoint(true)` のときだけ開き、既定は開かない。
- **利用者:** Anonymous は `with_anonymous` で許す・許さないを選ぶ（既定は許す。証明書の
  信頼が要るので、知らない機械からは入れない）。UserName は、利用者が渡す
  `UserAuthenticator` で照合する（`OpcUaServer::start_with_handlers`）。banto-opcua は
  資格情報を持たない。書き込みのハンドラー（ADR-0004）と同じく、判断は利用する側にある。
  照合は 10 秒で打ち切る。断った理由（利用者名が無い・パスワードが違うなど）は
  クライアントへは区別せず `BadUserAccessDenied` で返し、サーバーの記録にだけ残す。
- **`None` のエンドポイントでは UserName を受け付けない。** パスワードが平文になる。
  `None` を開くなら Anonymous を許すこと（許さないなら起動はエラー）。
- **トラスト:** クライアントの証明書は `pki_dir` の信頼の一覧で判断する。初めて来た
  証明書は拒否の一覧に置かれ、管理者が `RunningServer::trust_list().trust(拇印)` で
  信頼に移すと、次の接続から受け付ける（再起動は要らない）。全部を信頼する
  `with_trust_all_client_certificates(true)` は試験用で、起動時に警告を出す。
- **状態の口:** `RunningServer::security_events()` が、直近 256 件の受け付けた・断った
  セッション（Anonymous・UserName、理由、SecurityPolicy、MessageSecurityMode）を返す。
  クライアントの証明書による拒否は async-opcua の中で起き、差し込む口が無いのでここには
  出ない。信頼していない証明書は信頼の一覧の拒否の側に、期限切れは証明書の有効期間に
  現れるので、利用する側はそれを画面に出す。
- **書き込みの主体:** UserName で照合を通したセッションの Write は
  `WritePrincipal::User(利用者名)` としてハンドラーへ渡す。

### 自分の証明書（ADR-0003 の続き）

- 鍵と証明書は async-opcua の起動時の生成ではなく、banto-opcua が先に用意する。
  SubjectAltName に ApplicationUri と、エンドポイントの公開ホスト・bind したアドレスを載せる。
- 既存の組は読み直して確かめる。読めない、ApplicationUri が違う、公開ホストが載って
  いない、有効期間の外、のどれかなら警告を出して作り直す。拇印が変わるので相手は
  信頼し直す。設定を変えたのに古い証明書のままでは、相手のホスト名の検査で必ず断られる。
- 有効期間は 5 年にする。async-opcua の既定の 1 年では、毎年すべての相手が信頼し直す。
- 置き場の操作（鍵の用意と信頼の一覧）はサーバーとクライアントで同じなので、新しい
  クレート `banto-opcua-pki` に置き、両方が同じ `TrustList` を公開する。

### クライアント（ADR-0005 の部品）

- `ClientConfig::with_security`・`with_user_name`・`with_pki_dir`・`with_application` で、
  サーバーと同じ SecurityPolicy・MessageSecurityMode・UserName で接続する。既定は従来の
  `None` / Anonymous のまま（相手は他社のサーバーで、`None` しか持たないこともある）。
  `None` で UserName は使えない。暗号化するなら `pki_dir` が要る（試験用の
  `with_trust_all_server_certificates(true)` のほか）。
- 接続の前に GetEndpoints を読み、設定に合うエンドポイントが無ければ
  `Error::EndpointNotFound`。一覧そのものは `OpcUaClient::endpoints` で返す（接続の
  画面で、選べる組とサーバーの証明書の拇印を見せる）。
- **サーバーの証明書は、セッションを開く前に自分の一覧と照らす。** GetEndpoints で
  受けた証明書が一覧に無ければ拒否の一覧へ置き（保留）、`ServerCertificateUntrusted` で
  返す。利用する側が拇印を確かめて `TrustList::trust` すると、次から通る。信頼していても
  有効期間・ホスト名・ApplicationUri が合わなければ `ServerCertificateInvalid`。接続の
  ときの検査は async-opcua が改めて行う。
- 断られたときは、`UserRejected`（利用者）と `ClientCertificateRejected`（サーバーが
  こちらの証明書を断った）に分ける。サーバーの証明書は先に通してあるので、証明書の
  失敗はサーバーの側の拒否とみなせる。

## 検討した代替案

- **案A（採用）: 上のとおり。** 既定が安全側で、資格情報と信頼の判断を利用する側が持つ。
  `None` を使う既存の利用者（Hub の診断用の公開、試験）は `with_none_endpoint(true)` を
  足す必要がある。
- **案B（不採用）: async-opcua の `DefaultAuthenticator` に利用者とパスワードの表を設定で渡す。**
  banto-opcua が資格情報を持つことになり、Hub の利用者管理と二重になる。ADR-0004 で書き込みの
  認可を利用する側に置いたのと揃わない。
- **案C（不採用）: クライアントは接続してから、StatusCode とサーバーの証明書の状態で理由を分ける。**
  サーバーはクライアントの証明書を要求の時点で確かめるので、両方が初対面だとサーバーの拒否が
  先に返り、クライアントはサーバーの証明書を見ないまま終わる。理由を正しく分けられず、
  サーバーの証明書も保留にならない。
- **案D（不採用）: サーバーの既定に `None` を残す。** 設定を変えずに LAN へ出すと平文・匿名の
  まま公開される。オーナーの指示（#54）どおり、既定では開かない。
- **案E（不採用）: クライアントの証明書による拒否を記録するため、async-opcua を fork で直す。**
  信頼の一覧と有効期間で利用する側は状態を出せるので、今は fork を増やさない。要るように
  なったら上流へ口を提案する。

## 帰結

- 公開 API に互換でない変更がある。サーバーの既定のエンドポイントから `None` が消えたので、
  `None` で接続する利用者はサーバーに `with_none_endpoint(true)` を足す。
- `pki_dir` を設定しないサーバーとクライアントは、信頼の一覧も起動ごとに空になる。
  本番では必ず設定する（Hub はデータディレクトリの下）。
- 証明書を作り直すと拇印が変わり、相手の信頼の一覧から外れる。ApplicationUri や
  公開ホストの変更、5 年の期限がその契機になる。利用する側は作り直しを画面で知らせる。
- 証明書の利用者トークン（X509）と発行されたトークンは受け付けない。必要になったら別の ADR。
- IPv6 のホスト名で接続するときの証明書のホスト名の検査は、プロセス内テストで確かめていない。
- 相互接続は Python asyncua で、暗号化したエンドポイントと UserName を確かめる。UaExpert での
  確認は手動、ソークと OPC Foundation CTT は後続。
