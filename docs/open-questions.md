# 未解明事項ログ

形式上の未確定事項を、推測で公開APIへ固定しないための記録である。新しい証拠が得られた場合は、対象ファイル、作成アプリ版、比較条件を追記する。

## ブロックチェックサム

- 状態: 未解明
- 観測: データありブロックは非0、空ブロックは0。
- 不一致を確認済み: 圧縮前後のCRC-32、Adler-32、zlib末尾Adler-32。
- 現在の扱い: 検証せず不透明な `u32` として保持。
- 追加観測: 匿名の短いストローク2件と部分マスクでも候補不一致。使用したCRCカタログの全32-bit variant、ヘッダー込み範囲、City/Farm/Metro/SpookyHash、MurmurHash2/3、FNV、Boost、xxHash、暗号学的ハッシュの32-bit部分にも一致しなかった。
- 互換性観測: 全チェックサムを0にした匿名ローカル複製をCLIP STUDIO PAINTで開き、表示を確認できた。別名保存後も0が保持された。
- 影響: 少なくとも検証したアプリ版では0を互換値にできる可能性が高い。writerでは既存値を保存し、再生成値0をopt-inにする余地がある。
- 次の調査: 0値文書を複数アプリ版で開き、編集後に再生成されたブロックとの関係も比較する。

## `BlockStatus` の意味

- 状態: 未解明
- 観測: 値は0または1だが、データ有無とは一致しない。要素数は常にブロック数と一致。
- 追加観測: 新規保存した匿名の最小ファイルでは、空ブロックを含めて全要素が1だった。短いストロークと部分マスクのデータブロックも1だった。
- オブジェクト単位の規則: 5サンプルの5,293外部オブジェクト・163,927ブロックでは、値0のオブジェクト1,322件、値1のオブジェクト3,971件で、同一オブジェクト内の混在は0件だった。追加の匿名生成・アプリ補助コンテナ137件でも混在は0件だった。
- 現在の扱い: 不透明な `u32` として各ブロックに保持し、全ブロック一致時は `BlockData::uniform_status` でも取得できる。値の意味は固定しない。
- 次の調査: 同じキャンバスで編集・保存・再保存を繰り返し、更新タイルとの相関を調べる。

## 12バイトのブロック属性

- 状態: 実ファイルで2配置を確認
- 観測: 通常描画は `channels=5`、部分レイヤーマスクは `channels=1`。どちらも `reserved=0`, `width=256`, `height=256` と解釈した展開長に一致。
- 現在の扱い: 生バイトを保持しつつ、上記フィールドのgetterも提供。
- 次の調査: 1-bit、異なるタイル寸法・チャンネル数のファイルを作成して比較する。

## DB参照はあるが外部チャンクがないID

- 状態: ラスター復元上の意味は解明済み
- 観測: 5ファイルの全外部参照列を横断し、未索引IDは `Offscreen.BlockData` の14,144/19,437件だけだった。全IDが40-byteで行ごとに一意、対応する属性は既知の5チャンネル8-bit配置・初期色0で、既定値は0が14,114件、1が30件だった。
- 結論: 共通sentinelや参照破損ではなく、外部タイルを省略して `Offscreen.Attribute` の既定値だけを使うスパース画像である。
- 現在の扱い: DB参照なし・未索引ID・外部データありを `RasterDataState` で区別し、前2状態は `is_default_filled()`、実データは `is_present()` で判定する。
- 残る調査: ID自体を行ごとに発行して索引しない設計理由は不明だが、読み取り結果には影響しない。

## レイヤー種別と特殊機能

- 状態: 補正レイヤーparameter、特殊定規metadata、2Dカメラは解明・実装済み、その他は部分的
- 観測: `LayerType` は複数の数値を取り、サンプル間でスキーマ列にも差がある。匿名のベクターレイヤーでは `LayerType=0`、`VectorObjectList` 1行、40-byteの外部ID、268-byteの未分類外部本体を確認した。同レイヤーの描画Mipmapだけでは実際の線を復元できない例だった。匿名の単一テキストでは `LayerType=0`、`TextLayerType=0`、UTF-8本文BLOB、1,029-byteの属性BLOB、属性version 1を確認し、追加配列はNULLだった。公開サンプルの補正レイヤー32件は `LayerType=4098` で、`FilterLayerInfo` kind 1～9をすべて末尾まで復号できた。定規サンプル18レイヤーではベクター定規参照8件と特殊manager 10件があり、9定規表の16定規と透視消失点chainを全件到達できた。
- 現在の扱い: レイヤー種別の元の整数値を保持し、ベクターは外部本体まで上限付きで取得する。テキストはUTF-8本文とオブジェクト別属性の境界まで検証するが、各属性の意味は不透明なバイト列として保持する。補正レイヤーは `Database::correction_layer` から9種の型付きparameterと元payloadを返し、未知kindはpayloadを保持する。定規は `Database::ruler_layer` で所有関係・chain・curve点・guide長、2Dカメラは `Database::camera_2d_layer` でsnapshot構造を検証する。
- 次の調査: ベクター本体の線・制御点・ブラシ属性と、テキストのフォント・段落・変形属性を差分比較する。3Dも1種類ずつ含む最小コーパスを作成する。定規はcurve header 4語と透視guide BLOBの意味を差分比較する。

## 1-bitラスターデータ

- 状態: 8-bitの1チャンネルは実ファイル確認済み、1-bitは未確認
- 観測: 匿名の白マスクと部分マスクを `Gray8` として復号し、部分マスクの1タイルが `1 × 256 × 256 = 65,536` バイトであることを確認。
- 追加観測: ラスターレイヤーの表現色をモノクロへ変更して保存しても、描画本体は従来どおり5チャンネル・各チャンネル8-bit、展開長327,680バイトだった。表現色だけでは1-bit格納にならない。
- 現在の扱い: 8-bitの1チャンネル配置は対応済み。`PixelPacking` が1-bitまたはmonochromeを示す場合は推測展開せず `UnsupportedRaster` を返す。
- 次の調査: 外部画像の読み込みなど別経路で1-bit格納を生成し、ビット順序、行パディング、既定色を比較する。

## `.cmc`

- 状態: 外側SQLite、Project、CanvasNode tree、ページ参照まで解明・実装済み
- 観測: 匿名生成1件と公開サンプル4件はすべて通常のSQLite 3だった。全件でProjectは1行、CanvasNodeはroot (`Type=0`) とページ (`Type=2`) からなり、計31ノードをrootから重複・循環なく到達できた。
- ページ参照: 全31ページの `LinkPath` は `.:pageNNNN.clip` 形式で、同じディレクトリの実在 `.clip` に解決できた。
- 現在の扱い: `CmcFile` がSQLiteサイズ・ノード数、Project件数、参照先、循環、複数親、到達性を検証する。未知linkはraw文字列を保持し、安全な単一ファイル形式だけを `page_file_name` / `page_path` で解決する。
- 残る調査: `Project` の印刷・綴じ・作品情報各列と、`CanvasNode` のmemo・警告flagの完全な意味は固定しない。

## アニメーションの未解釈部分

- 状態: Track chain、primary/secondary FCurve、inline `TrackValueMap`、2Dカメラまで対応
- 観測: 既存コーパス5件の291トラックからprimary 270曲線・12,347キーを復号した。`FirstTrack` / `TrackNextIndex` は全件を一度ずつ通る終端付きchainだった。`1000` はnon-cel folder 42/42、`2001` はstatic image 45/45、`2003` はpaper 5/5、`4000` は `PlayTime` 4/4、`4001` はaudio 4/4と対応した。`2001` の内訳はraster 42と `ResizableImageInfo` を持つresizable image 3で、全件の曲線とvalue entryが空だった。`2000` は複数の `ImageCelName` を持つ。補間、左右傾き、任意タグもキー数一致を確認した。`TrackValueMap` は全291行でrecord境界まで一致し、type 0の倍精度値とtype 2の文字列・整数値を確認した。`2000` の整数値は対応FCurve値と191/191で一致した。secondary `0110binc` の値recordは先頭fieldの `Int32[]` / `Name` / `End` metadata headerでschema記述と区別でき、後続headerはfield種別により残り2語が変化する。実値は37曲線・37キー（cel 32、audio 5）で、対応するprimaryと全フィールドが37/37で完全一致した。`4000` の4行は対象layerがtype 0のleafでtype 256のroot直下にあり、2Dカメラ用trackではないことも確認した。
- 2Dカメラ: 匿名最小差分3件で `LayerType=512` と `TrackKind=2005` を確認した。value map type 3はdouble XYで、center・position・rotation・scale・opacityの5現在値を持つ。位置を `(15, 3)` 移動するとprimary/secondary双方へcenter X/Y、position X/Y、rotation、scaleの6曲線が現れ、snapshotのpositionと4隅も同量移動した。
- 現在の扱い: chainを検証してnext ID、raw track kind、primaryの単精度曲線、secondaryの倍精度曲線、型付きvalue mapを公開する。`1000` / `2000` / `2001` / `2003` / `2005` / `4000` / `4001` は確認済みhelperを持つ。2Dカメラはlayer snapshot、type 3の2次元値、軸付きcurveを型付きで返す。未知value typeとsnapshot raw payloadは保持する。各 `Value` の単位とsnapshotの未命名11語は固定しない。従来の `CelTrack` は先頭のprimary `ImageCelName` を使う。
- 次の調査: [2Dカメラ追加差分の手動手順](manual-2d-camera-extended-fixtures.md)でscale・rotation・opacityを1項目ずつ変え、snapshotの未命名11語を比較する。出力サイズと3D transformは別の匿名最小差分を作成する。
- GUI検証補足: 2026-07-24に手動で匿名のbaseline・camera folder・position keyframe差分を作成し、解析を完了した。生成物と解析scriptは `tester/` のみに保持する。

## タイムラプス内部ストリーム

- 状態: record/blob連結、BLOB展開、内部WebP key/delta frame索引、記録順序まで対応
- 観測: 2サンプルの9 BLOBは `WEBP` encoder、big-endian長付きzlibで、DBの圧縮・展開サイズと連続offsetが一致した。計76,799件すべてが28-byte little-endian headerとRIFF/WebPの連続recordで、長さ、1-origin sequence、`EncoderSequence`、末尾境界が一致した。`GMIK` 3,100件は全件full canvasで隣接間隔は最大30、`GMID` 73,699件は全件parameterを左上原点とするcanvas内patchだった。先頭WebP chunkは `VP8 ` / `VP8X` だった。
- 時刻の結論: 3つのタイムラプス表に時刻・間隔列はなく、headerのreserved値も全件0だった。ファイルが保持するのはrecord順序であり、wall-clock時刻や休止時間は復元できない。動画長は書き出し時にアプリ側で選択されるため、sequenceから実時間を推測しない。
- `GMIK` parameter: 直前・直後deltaとの原点一致は約18%、patch内率も約30%に留まった。先頭58 key間の画素差分でも変化領域原点との一致はなく、parameter位置が実際に変化したのは33件だったため、意味を固定できない。
- 現在の扱い: BLOB単位の上限付き読み取り・ストリーミング展開に加え、画像payloadを保持しないframe indexを公開する。raw FourCC、2つのreserved値、2つのraw parameter、RIFF offset・長、sequence、WebP先頭chunk・寸法を保持し、key/delta判定とdelta originも返す。
- 次の調査: 匿名の短時間記録で描画位置と操作種別を1項目ずつ変え、`GMIK` 側parameterだけを比較する。

## GUI検証の運用

2026-07-23に匿名の最小差分コーパスを作成し、短いRGBAストローク、白レイヤーマスク、部分レイヤーマスク、モノクロ表現色、ベクターストローク、単一テキストを検証した。生成ファイル、解析スクリプト、出力は `tester/` のみに置き、Git・公開crate・ドキュメントへ作品名、レイヤー名、本文、ユーザーパスを残さない。
