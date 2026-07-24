# 高レベル文書モデル

`sqlite` featureを有効にすると、`ClipFile::read_document()` または `Document::load()` で文書モデルを構築できる。対象は、5サンプルと複数の公開研究実装で関係を確認できた中核フィールドに限定している。

## 型と責務

- `Project`: 内部形式version、任意の作品名、主キャンバスID
- `Canvas`: ID、単位、幅・高さ、解像度、ルート・現在レイヤーID
- `CanvasPreview`: キャンバスID、形式値、寸法、エンコード済みプレビュー
- `VectorDataSource`: ベクターデータ行、所有キャンバス・レイヤーID、不透明な外部ID
- `RulerLayerData` / `Ruler`: ベクター定規参照、9種の特殊定規、曲線点・透視消失点chain
- `TextLayerData` / `TextObjectData`: UTF-8本文、所有レイヤー、形式値、オブジェクト別の不透明な属性
- `CorrectionLayerData` / `Correction`: 補正レイヤーの形式値、9種の型付きparameter、元の属性payload
- `Animation` / `Timeline` / `AnimationTrack` / `AnimationCurve` / `AnimationTrackValueEntry` / `CelTrack`: 再生範囲、fps、raw track kind、レイヤー対応、汎用FCurve、現在値、セル選択キー
- `Camera2DLayerData` / `Camera2DTransform` / `Camera2DTrackValues`: 2Dカメラfolder、現在transform snapshot、軸付きcurveと保存位置のtrack値
- `TimeLapse` / `TimeLapseManager` / `TimeLapseRecord` / `TimeLapseBlob` / `TimeLapseFrame`: canvasごとの記録、連続BLOB、内部WebP frame索引
- `CmcFile` / `CmcNode`: standalone `.cmc` のProject metadata、検証済みページtree、raw・安全解決済みページ参照
- `ClipWriter` / `EditableDatabase` / `WriteSummary` / `BlockWriteSummary`: 新規出力へ限定した低レベル再構築、書き込み可能SQLite複製、既存BlockData tileの限定的な再圧縮、結果のサイズ・offset
- `Layer`: 名前、種類、合成、可視性、不透明度、ロック、クリッピング、マスク、兄弟・子・Mipmap参照
- `LayerTree`: ルートから再構成した子IDの順序と、到達不能なレイヤーID
- `Document`: 上記の所有とID検索

`LayerTree` は再帰型ではなく、`children_of(layer_id)` で順序付きの子IDを返す。これにより、敵対的な深い入力をRustのコールスタックへ載せず、利用者も再帰・反復のどちらかを選べる。

## 合成モード

`Layer::blend_mode()` は `LayerComposite` を `BlendMode` として返す。匿名化したローカルcorpus 91ファイルを読み取り専用で走査し、次の29値を実データで確認した。

| raw値 | `BlendMode` 定数 | raw値 | `BlendMode` 定数 |
|---:|---|---:|---|
| 0 | `NORMAL` | 15 | `SOFT_LIGHT` |
| 1 | `DARKEN` | 16 | `HARD_LIGHT` |
| 2 | `MULTIPLY` | 17 | `VIVID_LIGHT` |
| 3 | `COLOR_BURN` | 18 | `LINEAR_LIGHT` |
| 4 | `LINEAR_BURN` | 19 | `PIN_LIGHT` |
| 5 | `SUBTRACT` | 20 | `HARD_MIX` |
| 6 | `DARKER_COLOR` | 21 | `DIFFERENCE` |
| 7 | `LIGHTEN` | 22 | `EXCLUSION` |
| 8 | `SCREEN` | 23 | `HUE` |
| 9 | `COLOR_DODGE` | 24 | `SATURATION` |
| 10 | `GLOW_DODGE` | 25 | `COLOR` |
| 11 | `ADD` | 26 | `BRIGHTNESS` |
| 12 | `ADD_GLOW` | 30 | `PASS_THROUGH` |
| 13 | `LIGHTER_COLOR` | 36 | `DIVIDE` |
| 14 | `OVERLAY` |  |  |

`known_name()` は確認済み値の英語名を返し、未知値では `None` を返す。未知値を含む任意の値は `from_raw()` で作成でき、`raw()` で元の整数を損失なく取得できる。したがって、定数の追加によって将来のCLIP STUDIO PAINT形式との前方互換性を閉じない。

`write` featureの`ClipFile::writer()`は、strict validation後の埋め込みSQLiteをprivateな書き込み可能DBへ複製する。利用者は`EditableDatabase::connection`で明示的にSQLを実行できる。再構築時は未知のSQLite列と変更しない外部本体を保持し、`CHNKHead`のSQLite位置と全`ExternalChunk.Offset`を修復する。`write_to_path`は既存パスを上書きせず、新規出力をflush・同期・再オープンしてcontainer、SQLite、external indexを検証する。完全な不透明本体差し替えに加え、`replace_block_bytes`は既存BlockDataの1ブロックをnative byte列から再圧縮し、`BlockChecksumMode::CspCompatible`で圧縮長prefix込みのAdler-32を生成する。画像・text・vector・animationの安全な既存構造編集も提供するが、任意の新規objectを生成する一般的なencoderではない。詳細は[書き込みガイド](writing.md)を参照する。

`CmcFile::open(path, limits)` は、`.clip` の `CSFCHUNK` 内部DBとは別のstandalone SQLiteである `.cmc` を読み取る。Projectが1行であること、CanvasNodeの正の一意ID、全child/sibling/selected参照、循環、複数親、rootからの到達性を検証する。`CmcFile::from_reader` も利用できるが、元ディレクトリがないため `page_path` は返さない。未知の `LinkPath` は保持し、観測済み `.:name` 形式かつディレクトリ区切りや親移動を含まない場合だけページファイル名・パスへ解決する。

## 検証

モデル構築時に次を確認する。

- 必須テーブル・列の存在
- レイヤー数、キャンバス寸法、木の深さの `Limits`
- `MainId` の正値・一意性
- レイヤーからキャンバスへの参照
- キャンバスのルート・現在レイヤー参照
- 子・兄弟参照の存在、循環、複数親
- 不透明度の0～256範囲と、正の有限な解像度

`Database::canvas_preview(canvas_id, limits)` は、指定キャンバスのプレビュー1件を読み取る。1件のエンコード済みサイズは `Limits::max_preview_bytes` で制限し、PNGシグネチャがある場合はSQLiteの幅・高さと先頭IHDRを照合する。未知の `ImageType` と非PNGデータは推測で捨てず、生の形式値・バイト列として返す。

`Database::vector_data_sources(layer_id, limits)` は、`VectorObjectList.LayerId` が一致する行を列挙する。件数は `Limits::max_vector_objects` で制限する。各外部本体は `ClipFile::read_vector_data` で `Limits::max_vector_data_bytes` を適用して取得できる。現時点ではベクター本体の意味を解釈せず、`VectorDataSource` の所有情報と外部ID、および元バイト列を保持する。

`Database::ruler_layers(limits)` はschema判定とSQLを内部へ隠し、定規を所有するlayerをID順に列挙・検証する。1 layerだけ必要な場合は `ruler_layer(layer_id, limits)` を使える。両APIとも `Layer.RulerVectorIndex` の所有layer・canvas、または `SpecialRulerManager` と各 `First*` / `NextIndex` chainを検証する。parallel、curve parallel、multiple curve、radial line、radial curve、concentric circle、guide、perspective、symmetryの9テーブルを型付きで返す。curve `PointData` はbig-endian header・件数・有限な座標・末尾境界を検証し、透視定規は消失点chainと `GuideNumber × GuideDataSize` を照合する。ベクター定規の線本体は既存の不透明なvector data APIから取得する。

`Database::text_layer(layer_id, limits)` は、`TextLayerString` をUTF-8として検証し、対応する `TextLayerAttributes` と組にして返す。追加オブジェクトの配列は、4-byte little-endian長と本体の繰り返しとして境界を検証する。文字列と属性の件数一致を必須とし、総バイト数を `Limits::max_text_bytes`、オブジェクト数を `Limits::max_text_objects` で制限する。属性、`TextLayerAddAttributesV01` の元配列、version値は失わず保持する。write時のobject追加では、後者がprimaryも含む同形式の配列であることと各itemのparameter 50がmain属性のobject IDと一致することも検証する。

`Database::correction_layers(limits)` はschema判定とSQLを内部へ隠し、補正レイヤーをID順に列挙・検証する。1 layerだけ必要な場合は `correction_layer(layer_id, limits)` を使える。両APIとも `Layer.FilterLayerInfo` のbig-endian kind・section長を検証する。確認済みkind 1～9を、明るさ・コントラスト、レベル補正、トーンカーブ、色相・彩度・明度、カラーバランス、階調反転、ポスタリゼーション、二値化、グラデーションマップとして返す。レベル値・曲線座標・色・不透明度のraw固定小数点word、元payload、未知kindのpayloadを保持し、1 payloadのバイト数とchannel・stop・point数を `Limits` で制限する。

`animation` featureの `Database::timelines(limits)` は、fpsと再生範囲を検証して全タイムラインを返す。`ClipFile::read_animation(database, limits)` は有効な `AnimationCutBank.FirstTimeLine` を優先し、`read_animation_for_timeline(database, timeline_id, limits)` は明示したタイムラインを読む。どちらも同じbankの全トラックを読み、`FirstTrack` から `TrackNextIndex` をたどって循環・欠落・到達不能・重複IDがないことも検証する。primary `TrackActionMixer` はSQLiteの外部ID索引から直接解決し、little-endian長付きzlibを上限付きで展開する。BINC文字列表から全 `FCurve` を列挙し、配列境界、有限・昇順の60 Hzキー時刻、`Frame` / `Value` と任意の `Tag` / `Interp` / slope / `ReviseConstant` の同数性を検証する。vector parameterの `Axis=X/Y` は `AnimationCurve::axis` で保持する。`AnimationTrackKind` はraw値を保持し、確認済みの `1000`（non-cel folder）、`2000`（image cel）、`2001`（static image）、`2003`（paper）、`2005`（2D camera）、`4000`（play time）、`4001`（audio）に判定ヘルパーと名前を持つ。トラックとレイヤーは16-byte UUIDで照合する。

既存互換の `CelTrack` は `TrackKind=2000` の先頭 `ImageCelName` 曲線を使う。複数曲線、`PlayTime`、`AudioPlayer` を含むprimary mixerの全曲線は `Animation::animation_tracks()` から取得する。各 `AnimationTrack` はinline `TrackValueMap` の有無と全entryも返す。mapはbig-endianのヘッダ・record長とUTF-16BE文字列の境界を検証し、確認済みのtype 0を `Float(f64)`、type 2を `IndexedText`、type 3を `Vector2` として返す。将来typeは判別値・文字列・payloadを `Unknown` に損失なく保持する。secondary `0110binc` はschema側の見かけ上の `FCurve` と値recordを区別し、値record先頭fieldの3語headerは先頭が `Int32[]`、残りが有効な文字列IDであることを検証する。観測例には `Name` / `End` と `ShiftBlend` / `AnimInfo` があり、特定の組み合わせへ固定しない。後者の `Double[]` 時刻・値・傾きは `SecondaryAnimationCurve` から `f64` のまま取得できる。secondary値recordは疎であるため、secondary mixer外部IDが存在しても曲線配列は空になり得る。

`Database::camera_2d_layer(layer_id, limits)` は `LayerType` のcamera bit、folder flag、元frame center、`Camera2DResizableImageInfo` のheader・point record宣言を検証する。`Camera2DTransform` は寸法、scale factor、度単位のrotation、position、image center、4隅を型付きで返し、未命名wordとraw payloadも保持する。`ClipFile::read_animation` はkind `2005` と対象camera layerをUUIDで照合し、保存時のタイムライン位置で評価された5個の必須値を `Camera2DTrackValues` へまとめる。track側のrotationは度、scale・opacityは百分率である。`Animation::camera_track_for_layer` からlayer単位で取得できる。

`Limits::max_animation_bytes` は圧縮・展開ミキサー、タイムライン名、2Dカメラsnapshot、`Limits::max_animation_items` はタイムライン、トラック、BINC文字列・配列、camera frame cornerの上限に使う。

`timelapse` featureの `Database::time_lapse(limits)` は、manager・record・blobの連結リストを再構成し、循環、共有、欠落、canvas所有、連続offsetを検証する。各 `TimeLapseBlob` は外部ID、raw `BlobType`、圧縮・展開サイズを保持する。`ClipFile::read_time_lapse_blob` は1 BLOBだけを上限付きで確保し、`copy_time_lapse_blob` はwriterへ展開する。どちらもbig-endian長付きzlib、DBの `BlobSizeCompressed`（4-byte長を含む）、実際の展開長を照合する。

`ClipFile::read_time_lapse_frame_index` はrecordの全BLOBを順に展開し、画像payloadを保持せず内部frame索引だけを返す。各frameについて28-byte little-endian record header、連番、record長、RIFF/WebP境界、先頭 `VP8 ` / `VP8X` chunkの寸法を検証する。`TimeLapseFrameKind` は `GMIK` / `GMID` をraw FourCCのまま保持しつつ、full key frameとdelta patchの判定も返す。`GMID` の2つのparameterはWebP patchの配置原点として `TimeLapseFrame::delta_origin()` から取得できる。reserved値と `GMIK` 側parameterも捨てずに保持する。

確認済みのタイムラプスDB列と内部frame headerにはwall-clock timestampや記録間隔がなく、sequenceだけが記録順を表す。書き出し動画長はアプリ側で別途選択されるため、Rust APIはsequenceを実時間へ変換しない。

ラスターデータの外部IDが `ExternalChunk` にない場合は、`Offscreen.Attribute` の既定値だけで画像を組み立てる。`RasterDataState::MissingReference` と `MissingExternalChunk` はDB上の記録差を保持しつつ、どちらも `is_default_filled()` が真になる。外部blockを復号した `Present` だけが `is_present()` を返す。

`Limits::max_time_lapse_blob_bytes` は1 BLOBの圧縮・展開サイズ、`Limits::max_time_lapse_items` はmanager・record・blob・frame数を制限する。record全体は数百MiBになり得るため、一括結合APIは設けない。

ルートから到達できないレイヤー行は、履歴・削除状態などの可能性を推測して破棄せず、`LayerTree::unreachable_layer_ids()` に残す。

## 前方互換性

`LayerType` はビットフラグとして複数用途を表し、`LayerComposite` にも将来値が追加され得る。このため `LayerKind` と `BlendMode` は閉じたenumにせず、`raw()` で元の整数を必ず返す。`BlendMode::known_name()` は未知値を明示するため `Option` を返す。`is_pixel()` などは、現在確認できたビットだけを判定する補助APIである。

ベクターは外部本体への安全な到達、定規は表間参照と特殊定規parameterまで対応したが、ベクター線本体・制御点・ブラシ属性はまだ解釈しない。テキストは本文と属性レコードの境界まで対応したが、フォント・段落・変形属性は未解釈である。補正レイヤーは9種の属性parameter、2Dカメラはlayer snapshot・現在値・軸付きcurveまで対応した。タイムラプスはfull key frameとdelta patchを含む内部WebP frame索引まで対応したが、`GMIK` 側parameterは未解釈である。タイムラプスの実時間はファイルに存在しないため復元対象にしない。3Dの詳細BLOBも同様に未解釈である。これらは元のDBへ `Database::connection()` で読み取りアクセスできるが、安定した意味モデルとしては、最小差分コーパスで検証後に追加する。
