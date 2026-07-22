# `.clip` ファイル形式の調査結果

調査日: 2026-07-21（最小差分検証: 2026-07-23）

この文書は公式仕様ではない。Git管理対象外のローカルコーパス5件を読み取り専用で比較した結果、匿名の最小差分ファイルで再検証した結果、公開されている独立実装から得た知見をまとめたものである。元ファイル名、作品名、レイヤー名、ローカルパスは記録しない。

## 結論

`.clip` は単純なSQLiteファイルではなく、次の3層からなるコンテナである。

1. `CSFCHUNK` で始まるビッグエンディアンの外側コンテナ
2. タイル画像、タイムライン、タイムラプス、音声などを収める `CHNKExta` 群
3. キャンバス、レイヤー、外部データ参照などを収める `CHNKSQLi` のSQLiteデータベース

5ファイルすべてで、`CHNKHead` → 0個以上の `CHNKExta` → `CHNKSQLi` → 空の `CHNKFoot` という順序、宣言サイズと実ファイルサイズの一致、SQLite位置の一致を確認した。

## 調査対象と集計

既存コーパス5件ではファイルサイズ37,590,980～351,500,428バイト、`Layer` 合計2,772行、`CHNKExta` 合計5,889件、SQLiteペイロード合計11,972,608バイトを観測した。個々の作品との対応が不要な集計値だけを本文に残す。

全SQLite DBのページサイズは4,096バイト、`ProjectInternalVersion` は `1.1.0` だった。ただし、これはCLIP STUDIO PAINT製品バージョンではなく、DB内部の値にすぎない。

## 外側コンテナ

整数は、特記しない限りビッグエンディアンの符号なし整数である。

### ルートヘッダー

| 相対位置 | サイズ | 内容 | 観測値 |
|---:|---:|---|---|
| `0x00` | 8 | マジック | ASCII `CSFCHUNK` |
| `0x08` | 8 | ファイルサイズ | 実ファイルサイズと一致 |
| `0x10` | 8 | 最初のチャンク位置 | 5件すべて `24` |

### 共通チャンク

各トップレベルチャンクは次の構造を持つ。

| サイズ | 内容 |
|---:|---|
| 8 | `CHNKHead` などのASCIIタグ |
| 8 | ペイロード長 |
| 可変 | ペイロード |

次のチャンク位置は `現在位置 + 16 + ペイロード長` で求められる。全サンプルで余白やアラインメントは観測されなかった。

### `CHNKHead`

ペイロードは全サンプルで40バイトだった。

| サイズ | 内容 | 観測値 |
|---:|---|---|
| 8 | 形式値 | `256` (`0x100`) |
| 8 | `CHNKSQLi` の絶対位置 | 実際のチャンク先頭と一致 |
| 8 | 識別子長 | `16` |
| 16 | ファイル識別子 | UUID v4と整合するビット配置の不透明値 |

識別子は現時点ではUUID型に固定せず、16バイトの不透明値として扱うのが安全である。

### `CHNKExta`

ペイロードの共通プレフィックスは次のとおり。

| サイズ | 内容 | 観測値 |
|---:|---|---|
| 8 | 識別子長 | `40` |
| 40 | 識別子 | `extrnlid` + 大文字16進32桁 |
| 8 | 本体長 | `ペイロード長 - 56` |
| 可変 | 本体 | 用途別形式 |

SQLiteの `ExternalChunk(ExternalID, Offset)` は、各 `CHNKExta` の識別子と絶対位置を保持する。5ファイル合計5,889行について、識別子はすべて一意で、実ファイル上の位置との不一致は0件だった。

`ExternalTableAndColumnName` には、外部参照を持ち得る次の列が宣言されていた。

- `Offscreen.BlockData`
- `VectorObjectList.VectorData`
- `Canvas3DModelBank.BankData`
- `Canvas3DModelLoader.ModelData`
- `Track.TrackActionMixer` / `Track.TrackActionMixer2`
- `CanvasItemBinary.ItemBinaryData`
- `Manager3DOd.SceneData`
- `ModelData3D.Layer3DModelData`
- `TimeLapseBlob.BlobData`

宣言されたテーブルの一部はサンプルDBに存在しない。したがってテーブルの固定セットを前提にしてはならない。

## 外部データ本体の分類

全5,889外部チャンクは、先頭シグネチャとSQLite参照を突き合わせることで次のように分類できた。

| 分類 | 5件合計 |
|---|---:|
| ブロックデータ | 5,293 |
| LE長 + zlib | 582 |
| BE長 + zlib | 9 |
| 生メディア | 5 |

- リトルエンディアン32-bit長の後にzlibストリームが続く形式は、`TrackActionMixer` と `TrackActionMixer2` に対応した。長はzlib部分のバイト数、すなわち外部本体長から4を引いた値だった。
- ビッグエンディアン32-bit長の後にzlibストリームが続く形式は `TimeLapseBlob.BlobData` に対応した。
- 生メディアではRIFF/WAVE、ID3付きMP3、MPEG Audioフレームを観測した。
- ブロックデータは、SQLiteの `Offscreen.BlockData` から参照されるものと一致した。

同じ「長＋zlib」でも用途により長のエンディアンが異なる点に注意が必要である。

## ブロックデータ

ブロックコンテナは、複数の `BlockDataBeginChunk` / `BlockDataEndChunk` ペアと、末尾の `BlockStatus`、`BlockCheckSum` からなる。ラベル文字列はUTF-16BEで、文字数は32-bit BEで保持される。

### データブロック

| サイズ | 内容 | 観測値・解釈 |
|---:|---|---|
| 4 | ブロック全長 | このフィールド自身からEndラベル末尾まで |
| 4 | Beginラベル文字数 | `19` |
| 38 | UTF-16BEラベル | `BlockDataBeginChunk` |
| 4 | タイル番号 | 0始まりの連番として扱える |
| 12 | タイル属性 | 常に `000500000000010000000100` |
| 4 | データ有無 | `0` または `1` |
| 4 | 外側データ長 | データありの場合のみ。内側長 + 4 |
| 4 | 内側データ長 | この値だけリトルエンディアン |
| 可変 | 圧縮データ | 全件zlib、先頭は `78 01` |
| 4 | Endラベル文字数 | `17` |
| 34 | UTF-16BEラベル | `BlockDataEndChunk` |

12バイトのタイル属性は `u16 channel_count = 5`, `u16 reserved = 0`, `u32 width = 256`, `u32 height = 256` と解釈すると観測結果と合うが、これは推定である。

| 全ブロック | データあり | 空 | 圧縮バイト合計 |
|---:|---:|---:|---:|
| 163,927 | 25,144 | 138,783 | 87,266,067 |

データありの25,144ブロックをすべて展開したところ、既存コーパスでは各ブロックが327,680バイト、すなわち `5 × 256 × 256` バイトだった。公開実装の解析と照合すると、先頭の65,536バイトがα、その後が4バイトインターリーブのB/G/R/未使用チャンネルである。Rust実装でも5サンプルから各1レイヤーをRGBAへ展開し、寸法・展開長・タイル境界を検証した。

匿名の256×256最小ファイルでレイヤーマスクを追加し、次を実ファイルで確認した。

- `LayerLayerMaskMipmap` がマスク用 `Mipmap` を参照し、`LayerType` と `LayerMasking` もマスク作成時に変化する。
- マスクの `Offscreen.Attribute` は1チャンネル8-bit配置を示し、白一色では既定値1から `Gray8` の255へ展開できる。
- 一部をマスクしたタイルのブロック属性は `channels=1, width=256, height=256`、展開長は65,536バイトで、0と255の両方を保持する。
- 同じレイヤーの描画本体は従来どおり5チャンネル配置であり、描画MipmapとマスクMipmapは独立して解決する必要がある。

これにより8-bitの1チャンネル配置は実ファイル検証済みになった。1-bit配置は引き続き未確認であり、8-bitと同一扱いにはしない。

匿名のラスターレイヤーで表現色をモノクロへ変更しても、描画本体は5チャンネル、各チャンネル8-bit、256×256タイルの展開長327,680バイトのままだった。CLIP STUDIO PAINT上の表現色設定だけを1-bit格納の生成条件としては扱えない。

### ステータスとチェックサム

`BlockStatus` と `BlockCheckSum` はともに、ラベルの後に `12`, ブロック数, `4`, ブロック数個の32-bit BE値を持つ。全ブロックコンテナで両方の要素数が実ブロック数と一致した。

チェックサムは、データありブロックでは全件非0、空ブロックでは全件0だった。一方、次の候補は25,144件すべてで不一致だった。

- 圧縮データのCRC-32 / Adler-32
- 展開データのCRC-32 / Adler-32
- zlib末尾のAdler-32
- 圧縮前後のCRC-32C、FNV-1a、Jenkins、Murmur3（seed 0）、xxHash32（seed 0）

匿名の短いストローク2件と部分マスクでも候補不一致を再確認した。新規保存した最小ファイルでは、データ有無にかかわらず `BlockStatus` が1になる例を観測したため、この値をデータ有無フラグとして扱うこともできない。

書き込み対応では、このアルゴリズムが未解明であることを無視してはならない。読み取り時は要素数と空ブロックの0値を検証し、非0値は不透明値として保持するのが妥当である。

## SQLite

`CHNKSQLi` のペイロードは先頭から `SQLite format 3\0` で始まる完全なSQLite DBであり、別ファイルへコピーすれば通常のSQLiteクライアントで開ける。

主要テーブルは次の役割を持つ。

| テーブル | 役割 |
|---|---|
| `Project` | 文書全体の設定、内部形式値 |
| `Canvas` | 幅、高さ、解像度、現在レイヤー、色・漫画・タイムライン設定 |
| `Layer` | 名前、種類、合成、可視性、木構造、各Mipmap参照 |
| `Mipmap` / `MipmapInfo` | 解像度系列と `Offscreen` の結び付け |
| `Offscreen` | タイル属性と外部ブロックID |
| `CanvasPreview` | PNGプレビューBLOB |
| `ExternalChunk` | 外部IDからファイル絶対位置への索引 |
| `ExternalTableAndColumnName` | 外部IDを格納し得るテーブル・列の宣言 |
| `Track` / `TimeLine` | アニメーションのトラックとタイムライン |
| `TimeLapse*` | タイムラプス記録。該当サンプルにのみ存在 |
| `ParamScheme` / `ElemScheme` | スキーマ記述・パラメーター情報 |

既存コーパス5件の `CanvasPreview` はすべて `ImageType=1` で、`ImageData` はPNGシグネチャとIHDRから始まり、SQLiteの `ImageWidth` / `ImageHeight` と一致した。匿名の最小差分ファイルでも同じ配置を確認した。公開APIではPNGを再エンコードせず、上限とIHDR整合性を検証した元バイト列を返す。

スキーマは固定ではない。対象5件だけでも次の差があった。

- テーブル数は20、22、25の3種類。
- タイムラプス有効サンプルだけに `TimeLapseBlob`, `TimeLapseManager`, `TimeLapseRecord` が存在。
- `Canvas` は109～119列、`Layer` は64または83列。
- あるサンプルでは `CanvasItemBinary` と `LayerObject` が存在しない。

このため、`SELECT *` の位置固定マッピングより、起動時の `PRAGMA table_info` と列名ベースの選択、欠落列の既定値、未知列の保持が望ましい。

Rustの高レベルモデルでは5サンプル合計2,772件の `Layer` を読み、`CanvasRootFolder`、`LayerFirstChildIndex`、`LayerNextIndex` から木を再構成した。5件とも全レイヤーがルートから一度ずつ到達し、循環・複数親・欠落参照は検出されなかった。木構造に含まれない行が将来現れても捨てず、到達不能IDとして公開する。

匿名の最小ベクターレイヤーでは `LayerType=0` と `VectorObjectList` 1行を確認した。`VectorData` は40-byteの外部IDで、`ExternalChunk` から268-byteの `CHNKExta` 本体へ解決できた。この本体は現在のシグネチャ分類では `Unknown` であり、線・制御点・ブラシ属性の境界は未確定である。同レイヤーには通常の描画Mipmapも存在したが、復号結果だけでは実際のベクターストロークを復元できなかった。このため公開APIは、`VectorObjectList` の所有情報を保持しつつ上限付きで元の外部本体を返し、意味解析は行わない。

## 未解明・未確認事項

- `BlockStatus` 値の意味とチェックサムアルゴリズム
- 1-bit、異なるタイル寸法・追加チャンネル配置
- ベクター本体、テキスト属性、3D、定規、特殊レイヤーの完全な意味
- `.cmc` の外側構造と複数ページ参照
- DBから参照されるが `ExternalChunk` に存在しない外部IDの意味
- アプリケーションのバージョン間での完全なスキーマ移行規則
- 安全な書き戻しに必要な全整合性条件

## 参考実装

解析時に以下を `tester/references/` へ浅くcloneした。このディレクトリはGit管理対象外であり、本crateへコードをコピーしていない。

- [rasensuihei/cliputils @ 7655c1d](https://github.com/rasensuihei/cliputils/tree/7655c1d78d8141bb68789e931e72406407cccbe3) — 外側チャンクとブロックの初期解析
- [dobrokot/clip_to_psd @ e8db68c](https://github.com/dobrokot/clip_to_psd/tree/e8db68c768f52ec91ba7530820f52537261cbad0) — SQLite、レイヤー、タイル展開の広範な実装
- [al3ks1s/clip-tools @ e1d3d7e](https://github.com/al3ks1s/clip-tools/tree/e1d3d7ed4701ac84e9220127daa73c2cf90dd803) — 読み書きAPI、DBモデル、形式メモ
- [ctrlcctrlv/libmarugarou @ f69cf09](https://github.com/ctrlcctrlv/libmarugarou/tree/f69cf091536357b96c9aa36ddc081e7869e9522a) — プレビュー抽出とzlib展開
- [castaneai/crista.py](https://gist.github.com/castaneai/79e3d13adeae68cc881ee78c89fb91c8) — 埋め込みSQLiteと `CanvasPreview` の短い実証

参考実装はいずれも未完成であり、相互に異なる仮定を含む。本書では、参考実装だけに依存する内容と手元サンプルで再確認できた内容を分けて記載した。
