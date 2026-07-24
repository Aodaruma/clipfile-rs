# ライブラリ実装方針

## 基本方針

初期の公開APIは、確度の高い低レベル形式から段階的に積み上げる。未解明な値を推測で捨てたり、書き込み時に0で埋めたりしない。

- 読み取りを先行し、書き込みは別段階にする。
- 巨大な`.clip`全体をメモリへ読み込まない。
- オフセットと長さは常にチェック付き整数演算で検証する。
- 既知の列・チャンクだけを解釈しつつ、未知データへ到達できる低レベルAPIを残す。
- SQLiteスキーマ、レイヤー種別、チャンク種別を非網羅的として扱う。
- 便利APIが依存関係やメモリ使用量を増やす場合はfeatureで分離する。

## 推奨モジュール境界

### 1. `container` — 実装済み

責務:

- `CSFCHUNK`、ルートサイズ、最初のチャンク位置の検証
- トップレベルチャンクのストリーミング走査
- `CHNKHead` と `CHNKExta` プレフィックスの解析
- SQLiteペイロードのストリーミング抽出
- 既知順序の厳密検証

この層は標準ライブラリだけで動作させる。現在の `ClipFile<R: Read + Seek>` がこの役割を担う。

### 2. `external` / `block` — 実装済み

責務:

- `BlockDataBeginChunk`, `BlockStatus`, `BlockCheckSum` の境界検証
- タイル番号、データ有無、圧縮長の取得
- 圧縮データを読み込まずに列挙できる `BlockRef`
- 明示的な上限付きzlib展開
- LE長付きzlib、BE長付きzlib、生メディアの判別

`ExternalObject`, `ExternalBody`, `BlockData`, `Block`, `BlockPayload` として実装した。チェックサムは `u32` の不透明値として公開し、未知アルゴリズムを「検証済み」と表現しない。

### 3. `database` — 実装済み（optional feature）

SQLite連携は `sqlite` featureとして切り離して実装した。

- `CHNKSQLi` を上限付きでSQLite管理メモリへ直接読み込み
- `pragma_table_xinfo` で実在列を確認
- 列名を明示したクエリを組み立てる
- 必須列欠落はエラー、任意列欠落は `None` または既定値
- `ExternalChunk` のID・位置を実ファイルと相互検証
- 未知テーブル・未知列の列挙APIを用意

`Database` は安全なスキーマ・索引APIに加え、高度な用途向けに読み取り専用の `rusqlite::Connection` も公開する。依存関係は既定featureへ含めない。

### 4. `raster` — 実装済み（optional feature）

用途と検証方法が明確なラスターレイヤー読み取りを `raster` featureとして実装した。

1. `Layer` → `Mipmap` → `MipmapInfo` → `Offscreen` を解決
2. `Offscreen.Attribute` から画像寸法、ブロックグリッド、チャンネル配置を解析
3. 外部IDから `CHNKExta` を取得
4. データありタイルだけをzlib展開
5. α + B/G/R/X をRGBAへ変換し、256×256グリッドへ配置

公開型は `DecodedTile` と `RasterImage` を分け、巨大画像を一括確保したくない利用者はタイル単位で処理できる。画像全体の確保量と1タイルの展開量は別々の `Limits` で制限する。現在は観測・検証できた8-bitの `(alpha, BGRA)` と1チャンネル配置を対象とし、1-bitや未知配置は明示的な未対応エラーにする。`image` crateとの変換は将来別featureへ分離できる。

ローカルコーパスでは5ファイル、19,437件の `Offscreen.Attribute` を全件解析し、各ファイルから少なくとも1つの実在ラスターレイヤーをRGBAへ展開した。全外部参照列のうち索引に存在しないIDは `Offscreen.BlockData` の14,144件だけで、すべて属性の既定値だけを使うスパース画像と確認した。DB参照なし・外部チャンク欠落・実データありは `RasterDataState` で区別し、前2状態は `is_default_filled()`、実データは `is_present()` で判定できる。

### 5. `model` — 実装済み

SQLiteスキーマを実行時確認する高レベル文書モデルを実装した。

- `Document`, `Project`, `Canvas`, `Layer`, `LayerTree`
- 合成モード、可視性、不透明度、クリッピング、マスクMipmap参照
- 9種の補正レイヤーparameterと未知kindのraw fallback
- 循環、複数親、欠落参照、深さ上限を検証する非再帰的な木構築
- 元のID・整数種別・フラグを保持する前方互換API

数値のレイヤー種別・合成モードは閉じたenumへ変換せず、`LayerKind` / `BlendMode` に元の `i64` を保持する。補正レイヤーは公開サンプル32件でkind 1～9を全件復号した。テキスト、ベクター、アニメーションの内容も、個別の形式を検証してから非網羅的サブモデルとして追加する。

ローカルコーパス5件では合計2,772レイヤーを読み、全件が各キャンバスのルートから重複なく到達することを確認した。

### 6. `animation` / `timelapse` — 部分実装済み（optional features）

通常アニメーションのうち、連番出力に必要なセル選択を検証可能な単位で実装した。

- `TimeLine` のfps、開始・終了・現在フレーム
- 有効な `AnimationCutBank.FirstTimeLine` の選択
- 同一bankの `TrackKind=2000` と対象レイヤーUUIDの対応
- `TrackActionMixer` のlittle-endian長付きzlib展開
- BINC文字列表と `ImageCelName` FCurveの `Frame` / `Value` / `Tag`
- 60 Hzキー時刻から表示フレームのセルタグを求める `CelTrack::cel_at_frame`
- 全track kindをraw値のまま保持する `AnimationTrack`
- `FirstTrack` / `TrackNextIndex` chainの全到達・循環検証とnext ID
- 確認済みfolder・image-cel・static-image・paper・play-time・audio kind helper
- primary action mixer内の全FCurve、補間、左右傾き、任意タグ
- `PlayTime` と `AudioPlayer` 曲線への上限付きアクセス
- inline `TrackValueMap` の境界検証と、倍精度値・indexed text・未知payloadの保持
- secondary mixer外部IDの有無
- secondary `0110binc` のfield metadata headerと倍精度FCurve
- `TimeLapseManager` / `Record` / `Blob` の連結リストとcanvas・offset検証
- big-endian長付きzlibの宣言サイズ照合、BLOB単位の上限付き読み取りとストリーミング展開
- 28-byte内部record、連番、RIFF/WebP境界、先頭chunk寸法のストリーミング索引
- full `GMIK` key frame、配置原点付き `GMID` delta patchの判定

secondary mixerで実値を持つ倍精度FCurveは構造単位で実装した。変形・カメラ等の完全な意味、タイムラプスの `GMIK` 側parameterは未実装である。タイムラプスのDBと内部headerにはwall-clock timestampがないため、公開APIは欠番のないsequenceだけを返し、実時間を推測しない。

### 7. `cmc` — 実装済み（`sqlite` feature）

`.cmc` は `CSFCHUNK` ではなくstandalone SQLiteであることを、匿名生成1件と公開サンプル4件で確認した。`CmcFile` はSQLiteサイズとノード数を制限し、Project 1行、CanvasNodeの正の一意ID、child/sibling/selected参照、循環、複数親、rootからの到達性を検証する。ページlinkはraw文字列を失わず、観測済み `.:name` 形式のうちパス区切り・絶対指定・親移動を含まない値だけを `.cmc` と同じディレクトリへ解決する。

### 8. `write` — 十分な知見を得た後

書き込みは読み取りAPIと別feature、別型にする。最低条件は次のとおり。

- ブロックチェックサムの0互換規則を複数アプリ版で検証（1版では開く・別名保存を確認済み）
- `ExternalChunk.Offset` と `CHNKHead` のSQLite位置の再計算
- SQLite内の全外部参照の保全
- 未知チャンク・未知列を失わないラウンドトリップ
- 一時ファイルへ全体を書き、検証成功後に置換するトランザクション方式
- CLIP STUDIO PAINT複数バージョンでの開き直し試験

現時点で公開の可変モデルを追加すると、不完全なファイルを書けるように見えてしまうため避ける。

匿名の最小文書では全 `BlockCheckSum` を0にしてもアプリで開け、別名保存後も0が保持された。このためwriterの初期実装では、未知チャンクと変更しない外部本体をbyte-for-byteで保存し、新規ブロックだけ0を用いる限定方式を候補とする。チェックサム値を正しいと偽るAPIにはせず、互換モードと検証対象バージョンを明示する。

## エラーと制限値

公開パーサーには設定可能な `Limits` を追加する。

- トップレベルチャンク数
- 外部ID長
- SQLiteサイズ
- 1外部チャンク当たりのブロック数
- 1ブロックの圧縮サイズ・展開サイズ
- キャンバス寸法と総ピクセル数
- レイヤー数、木構造の深さ
- `.cmc` のCanvasNode数
- 補正レイヤー1件の属性byte数、channel・stop・point数

構造エラー、未対応形式、制限超過、I/Oエラーは別variantにする。破損データを暗黙に読み飛ばすモードは既定にせず、将来追加する場合も `RecoveryOptions` の明示を要求する。

## テスト戦略

- 小さな合成バイト列で全境界条件を単体テストする。
- 再配布可能な最小`.clip`フィクスチャを別途作成し、出自と作成アプリ版を記録する。
- `tester/data/` のローカルコーパスで、チャンク数、DB位置、SQLite整合性、全ブロック境界を回帰確認する。
- `cargo-fuzz` でルート、チャンク、ブロック、属性パーサーを個別にfuzzする。
- 公開実装との相互比較は補助とし、最終的には構造不変条件とCLIP STUDIO PAINTでの実ファイル確認を基準にする。

## 直近のマイルストーン

1. `0.1.x`: 現在の読み取りAPI、実コーパス回帰、公開APIの整理
2. `0.2.x`: 実装済みのプレビューと8-bitマスクを固め、1-bitタイルを検証
3. `0.3.x`: テキスト・ベクターを検証可能な単位で追加（ベクター外部本体とUTF-8テキスト本文への上限付きアクセスは実装済み）
4. `0.4.x`: アニメーション・タイムラプスの読み取り（primary FCurveとタイムラプスBLOBの安全な到達は実装済み）
5. 以降: 書き込み条件の解明と、損失のないラウンドトリップ設計

SemVer上は `0.x` でも利用者への影響を抑えるため、既存の低レベル型を先に安定させ、高レベル型は `#[non_exhaustive]` とfeatureで段階導入する。
