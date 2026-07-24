# 高レベル文書モデル

`sqlite` featureを有効にすると、`ClipFile::read_document()` または `Document::load()` で文書モデルを構築できる。対象は、5サンプルと複数の公開研究実装で関係を確認できた中核フィールドに限定している。

## 型と責務

- `Project`: 内部形式version、任意の作品名、主キャンバスID
- `Canvas`: ID、単位、幅・高さ、解像度、ルート・現在レイヤーID
- `CanvasPreview`: キャンバスID、形式値、寸法、エンコード済みプレビュー
- `VectorDataSource`: ベクターデータ行、所有キャンバス・レイヤーID、不透明な外部ID
- `TextLayerData` / `TextObjectData`: UTF-8本文、所有レイヤー、形式値、オブジェクト別の不透明な属性
- `Animation` / `Timeline` / `AnimationTrack` / `AnimationCurve` / `AnimationTrackValueEntry` / `CelTrack`: 再生範囲、fps、raw track kind、レイヤー対応、汎用FCurve、現在値、セル選択キー
- `TimeLapse` / `TimeLapseManager` / `TimeLapseRecord` / `TimeLapseBlob` / `TimeLapseFrame`: canvasごとの記録、連続BLOB、内部WebP frame索引
- `Layer`: 名前、種類、合成、可視性、不透明度、ロック、クリッピング、マスク、兄弟・子・Mipmap参照
- `LayerTree`: ルートから再構成した子IDの順序と、到達不能なレイヤーID
- `Document`: 上記の所有とID検索

`LayerTree` は再帰型ではなく、`children_of(layer_id)` で順序付きの子IDを返す。これにより、敵対的な深い入力をRustのコールスタックへ載せず、利用者も再帰・反復のどちらかを選べる。

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

`Database::text_layer(layer_id, limits)` は、`TextLayerString` をUTF-8として検証し、対応する `TextLayerAttributes` と組にして返す。追加オブジェクトの配列は、4-byte little-endian長と本体の繰り返しとして境界を検証する。文字列と属性の件数一致を必須とし、総バイト数を `Limits::max_text_bytes`、オブジェクト数を `Limits::max_text_objects` で制限する。属性、追加属性、version値は意味を決め付けず元の値を保持する。

`animation` featureの `Database::timelines(limits)` は、fpsと再生範囲を検証して全タイムラインを返す。`ClipFile::read_animation(database, limits)` は有効な `AnimationCutBank.FirstTimeLine` を優先し、同じbankの全トラックを読む。`FirstTrack` から `TrackNextIndex` をたどり、循環・欠落・到達不能・重複IDがないことも検証する。primary `TrackActionMixer` はSQLiteの外部ID索引から直接解決し、little-endian長付きzlibを上限付きで展開する。BINC文字列表から全 `FCurve` を列挙し、配列境界、有限・昇順の60 Hzキー時刻、`Frame` / `Value` と任意の `Tag` / `Interp` / slope / `ReviseConstant` の同数性を検証する。`AnimationTrackKind` はraw値を保持し、確認済みの `1000`（non-cel folder）、`2000`（image cel）、`2001`（static image）、`2003`（paper）、`4000`（play time）、`4001`（audio）に判定ヘルパーを持つ。トラックとレイヤーは16-byte UUIDで照合する。

既存互換の `CelTrack` は `TrackKind=2000` の先頭 `ImageCelName` 曲線を使う。複数曲線、`PlayTime`、`AudioPlayer` を含むprimary mixerの全曲線は `Animation::animation_tracks()` から取得する。各 `AnimationTrack` はinline `TrackValueMap` の有無と全entryも返す。mapはbig-endianのヘッダ・record長とUTF-16BE文字列の境界を検証し、確認済みのtype 0を `Float(f64)`、type 2を `IndexedText` として返す。将来typeは判別値・文字列・payloadを `Unknown` に損失なく保持する。secondary `0110binc` はschema側の見かけ上の `FCurve` と、先頭fieldに `Int32[]` / `Name` / `End` metadata headerを持つ値recordを区別する。後者の `Double[]` 時刻・値・傾きは `SecondaryAnimationCurve` から `f64` のまま取得できる。secondary値recordは疎であるため、secondary mixer外部IDが存在しても曲線配列は空になり得る。

`Limits::max_animation_bytes` は圧縮・展開ミキサーとタイムライン名、`Limits::max_animation_items` はタイムライン、トラック、BINC文字列・配列の上限に使う。

`timelapse` featureの `Database::time_lapse(limits)` は、manager・record・blobの連結リストを再構成し、循環、共有、欠落、canvas所有、連続offsetを検証する。各 `TimeLapseBlob` は外部ID、raw `BlobType`、圧縮・展開サイズを保持する。`ClipFile::read_time_lapse_blob` は1 BLOBだけを上限付きで確保し、`copy_time_lapse_blob` はwriterへ展開する。どちらもbig-endian長付きzlib、DBの `BlobSizeCompressed`（4-byte長を含む）、実際の展開長を照合する。

`ClipFile::read_time_lapse_frame_index` はrecordの全BLOBを順に展開し、画像payloadを保持せず内部frame索引だけを返す。各frameについて28-byte little-endian record header、連番、record長、RIFF/WebP境界、先頭 `VP8 ` / `VP8X` chunkの寸法を検証する。`TimeLapseFrameKind` は `GMIK` / `GMID` をraw FourCCのまま保持しつつ、full key frameとdelta patchの判定も返す。`GMID` の2つのparameterはWebP patchの配置原点として `TimeLapseFrame::delta_origin()` から取得できる。reserved値と `GMIK` 側parameterも捨てずに保持する。

確認済みのタイムラプスDB列と内部frame headerにはwall-clock timestampや記録間隔がなく、sequenceだけが記録順を表す。書き出し動画長はアプリ側で別途選択されるため、Rust APIはsequenceを実時間へ変換しない。

ラスターデータの外部IDが `ExternalChunk` にない場合は、`Offscreen.Attribute` の既定値だけで画像を組み立てる。`RasterDataState::MissingReference` と `MissingExternalChunk` はDB上の記録差を保持しつつ、どちらも `is_default_filled()` が真になる。外部blockを復号した `Present` だけが `is_present()` を返す。

`Limits::max_time_lapse_blob_bytes` は1 BLOBの圧縮・展開サイズ、`Limits::max_time_lapse_items` はmanager・record・blob・frame数を制限する。record全体は数百MiBになり得るため、一括結合APIは設けない。

ルートから到達できないレイヤー行は、履歴・削除状態などの可能性を推測して破棄せず、`LayerTree::unreachable_layer_ids()` に残す。

## 前方互換性

`LayerType` はビットフラグとして複数用途を表し、`LayerComposite` にも将来値が追加され得る。このため `LayerKind` と `BlendMode` は閉じたenumにせず、`raw()` で元の整数を必ず返す。`is_pixel()` などは、現在確認できたビットだけを判定する補助APIである。

ベクターは外部本体への安全な到達まで対応したが、線・制御点・ブラシ属性はまだ解釈しない。テキストは本文と属性レコードの境界まで対応したが、フォント・段落・変形属性は未解釈である。アニメーションはprimary/secondary mixerのFCurveとinline value map、タイムラプスはfull key frameとdelta patchを含む内部WebP frame索引まで対応したが、カメラ・変形の意味付けと `GMIK` 側parameterは未解釈である。タイムラプスの実時間はファイルに存在しないため復元対象にしない。3D、定規の詳細BLOBも同様に未解釈である。これらは元のDBへ `Database::connection()` で読み取りアクセスできるが、安定した意味モデルとしては、最小差分コーパスで検証後に追加する。
