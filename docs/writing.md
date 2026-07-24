# `.clip` の書き込み

`write` featureは、既存の`.clip`を検証してから別の新規ファイルへ再構築する低レベルAPIを提供する。既定featureには含まれない。

```toml
[dependencies]
clipfile = { version = "0.5", features = ["write"] }
```

## 基本API

`ClipFile::writer()` は入力をstrict validationし、埋め込みSQLiteをprivateな書き込み可能メモリDBへ複製する。入力ファイル自体は変更しない。

```rust,no_run
use std::fs::File;

use clipfile::ClipFile;

let mut clip = ClipFile::open(File::open("input.clip")?)?;
let mut writer = clip.writer()?;

let changed = writer.database().connection().execute(
    "UPDATE Layer SET LayerName = ?1 WHERE MainId = 42",
    ["New name"],
)?;
assert_eq!(changed, 1);

let summary = writer.write_to_path("new-output.clip")?;
println!("output bytes: {}", summary.output_file_size());

# Ok::<(), Box<dyn std::error::Error>>(())
```

出力先がすでに存在する場合、`write_to_path` は上書きせずエラーにする。書き込み、flush、同期、再オープン後のcontainer validation、SQLite `quick_check`、`ExternalChunk` index検証まで成功した場合だけ完了する。途中で失敗した新規ファイルは削除する。

変更なしで再構築した場合は、SQLite pageと全外部本体を含めてbyte-for-byte同一になることを単体テストとローカル実ファイル5件で確認している。

## 外部本体

`replace_external_body` は既存の`CHNKExta` identifierに対して、headerを除く完全な外部本体を置き換える。長さが変わる場合は後続chunk位置、SQLite位置、全`ExternalChunk.Offset`を再計算する。

```rust,no_run
# use std::fs::File;
# use clipfile::ClipFile;
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
# let mut writer = clip.writer()?;
# let external_id = b"existing identifier";
# let complete_body = Vec::<u8>::new();
writer.replace_external_body(external_id, complete_body)?;
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

このAPIは外部本体を不透明なbyte列として扱う。ブロック、圧縮payload、チェックサム、タイムラプスの宣言サイズなどを生成・修復しない。置換内容と関連SQLite列の整合性は呼び出し側の責務である。1本体の上限は`Limits::with_max_write_external_body_size`で設定できる。

`add_external_body` は、呼び出し側が指定した未使用identifierと完全な本体から、新しい`CHNKExta`をSQLite chunkの直前へ追加する。`ExternalChunk`の索引行と絶対offsetは書き込み時にprivateなSQLite複製へ追加され、`EditableDatabase`自体は変更しない。source、編集用索引、pending置換・追加のいずれかとidentifierが重複する場合は拒否する。

```rust,no_run
# use std::fs::File;
# use clipfile::ClipFile;
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
# let mut writer = clip.writer()?;
# let unique_external_id = b"caller-provided unique identifier";
# let complete_body = vec![0_u8; 16];
writer.add_external_body(unique_external_id, complete_body)?;
println!("pending additions: {}", writer.addition_count());
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

この低レベルAPIはidentifierや本体の用途別encodingを生成せず、参照元SQLite rowも呼び出し側がtransactionで設定する。失敗時にpending追加を取り消すには`remove_external_addition`を使う。ライブラリ内のsemantic writerは、観測済み形式に沿った衝突しないidentifierを生成し、複数本体やSQLite更新の途中で失敗した場合にstage済み追加をrollbackする。`write_to_path`は追加後のcontainerと`ExternalChunk`索引も再オープンして検証する。identifier、本体、top-level chunk数には対応する`Limits`が適用される。

## 既存ラスターブロックの再エンコード

`replace_block_bytes` は、既存の `BlockData` 外部本体に含まれる1ブロックを、展開済みnative byte列からzlib再圧縮する。byte数は既存parameterの `channels × width × height` と完全一致する必要がある。対象以外の圧縮payload、ブロックindex・parameter・status・checksumは保持し、空ブロックへ初めてpayloadを設定することもできる。

`BlockCheckSum` は、4-byte little-endianの圧縮長と、それに続くzlib圧縮byte列を連結した範囲のAdler-32である。通常は `BlockChecksumMode::CspCompatible` を指定する。従来の `BlockChecksumMode::Zero` も、ローカル検証したアプリ版が0を受理する互換modeとして残しているが、新規コードではCSP互換生成を推奨する。checksum値自体は `BlockCheckSum` 配列へ32-bit big-endianで保存される。

```rust,no_run
use clipfile::{BlockChecksumMode, ClipFile};

# use std::fs::File;
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
# let external_id = b"existing block-data identifier";
# let block_index = 0;
# let native_tile_bytes = vec![0_u8; 5 * 256 * 256];
let mut writer = clip.writer()?;
let block = writer.replace_block_bytes(
    external_id,
    block_index,
    native_tile_bytes,
    BlockChecksumMode::CspCompatible,
)?;
println!("compressed bytes: {}", block.compressed_size());
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

この処理は対象外部本体を一度メモリへ読み込み、再構築した完全な本体を保留するため、必要量は `Limits::with_max_write_external_body_size` で制限される。同じidentifierへの複数回の呼び出しは直前の保留結果へ積み重なり、途中の呼び出しが失敗した場合は直前の有効な保留結果を維持する。

読み取りAPIで最初の実在タイルを選び、native byteを反転して新規ファイルへ書く最小例は `examples/invert_first_tile.rs` にある。

```console
cargo run --features "write,raster" --example invert_first_tile -- input.clip new-output.clip 42
```

最後の引数はlayer IDである。`cargo run --features raster --example inspect -- input.clip --raster` の `decoded layer ...` から候補を確認できる。この例は低レベルbyte APIの実演であり、色チャンネルだけでなくnative alpha等も反転する。

### Plain raster layerのtemplate clone

`clone_raster_layer_from_template` は、同じcanvasにある検証済みのplain leaf raster layerをtemplateにし、完全なRGBA8/Gray8 pixel列から新しいlayerを親layerの先頭へ追加する。`LayerType = 1`、layer maskなし、1本のrender mipmap chainとrender thumbnailを持つtemplateだけを受理する。

`Layer`、`Mipmap`、`MipmapInfo`、`Offscreen`、`LayerThumbnail`の未知列はtemplateからstorage classごと保持する。一方でrow identity、意味上のID、layer UUID、所有参照、tree link、外部IDは再生成する。100% base renderだけをCSP互換チェックサム付きBlockDataとして追加する。派生mipmapとthumbnail atlasには新しい未索引IDを設定し、templateの古いcacheを参照させない。canvas previewは再生成しない。

```rust,no_run
# use std::fs::File;
use clipfile::{ClipFile, Limits, PixelFormat};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
# let rgba_pixels = vec![0_u8; 256 * 256 * 4];
let mut writer = clip.writer()?;
let layer_id = writer.clone_raster_layer_from_template(
    42,
    1,
    "New raster",
    PixelFormat::Rgba8,
    rgba_pixels,
    Limits::default(),
)?;
println!("new layer: {layer_id}");
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

template画像を復号し、異なるpixel列を与える実行例は `examples/clone_raster_layer.rs` にある。

## Text

`EditableDatabase::replace_text_object_text` は、既存text objectの本文だけを置換する。styleやrun情報を含む属性BLOBの完全なencoderは未確定であるため、対応する各文字のUTF-8 byte幅とUTF-16 code unit幅がすべて同じ場合に限定する。総byte数だけが一致して文字境界が異なる置換は拒否する。

```rust,no_run
# use std::fs::File;
use clipfile::{ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let mut writer = clip.writer()?;
let old = writer.database().replace_text_object_text(
    42,
    0,
    "Hello",
    Limits::default(),
)?;
println!("replaced {old:?}");
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

`EditableDatabase::add_text_object_from_template` は、既存objectのstyle/layout属性を複製して同じレイヤーの末尾へobjectを追加する。`TextLayerStringArray`、`TextLayerAttributesArray`、primary分も含む `TextLayerAddAttributesV01` の3配列を検証し、後者2つのparameter 50へ文書内で一意な新IDを設定してから、1回のSQL更新で同期する。本文にはtemplateと文字ごとに同じUTF-8/UTF-16幅が必要である。

```rust,no_run
# use std::fs::File;
use clipfile::{ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let mut writer = clip.writer()?;
let added = writer.database().add_text_object_from_template(
    42,
    0,
    "World",
    Limits::default(),
)?;
println!("new object: {}, id: {}", added.object_index(), added.identifier());
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

追加objectのgeometryもtemplateから複製されるため、初期位置は重なりうる。BBOX/quadの座標系はまだ確定しておらず、このAPIは自動移動を行わない。属性のゼロ生成と異なる文字幅へのrun再構築は未対応である。

`remove_text_object` は、文字列・main属性・additional属性の対応するitemを1回のSQL更新で削除する。primary（index 0）の削除時は次のobjectをprimary列へ昇格する。text layerを空にする表現は保守的に採用せず、最後の1 objectの削除は拒否する。

```rust,no_run
# use std::fs::File;
# use clipfile::{ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let mut writer = clip.writer()?;
let removed = writer.database().remove_text_object(
    42,
    1,
    Limits::default(),
)?;
println!("removed {:?}", removed.text());
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Vector

vector stroke全体を新規生成するserializerは未確定である。`replace_vector_data_body` は、`Database::vector_data_sources` が返した型付き参照が編集用DBでも同じrowとidentifierを指すことを検証してから、完全な外部本体を置換する。これはstroke単位のsemantic encoderではなく、意図的に明示したopaque境界である。

既存bodyが検証済みの92-byte stroke header / 88-byte point layoutだけで構成される場合、`translate_vector_data` は全point位置とstroke/point bounding boxを整数canvas単位で平行移動する。brush、pressure、opacity、flags、未知byteは保持し、別layoutは変更前に拒否する。

```rust,no_run
# use std::fs::File;
# use clipfile::{ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let database = clip.open_database()?;
let source = database
    .vector_data_sources(42, Limits::default())?
    .into_iter()
    .next()
    .ok_or("no vector data")?;
let replacement_body = std::fs::read("vector-body.bin")?;
let mut writer = clip.writer()?;
writer.replace_vector_data_body(&source, replacement_body, Limits::default())?;
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

同じ検証済みlayoutでは、stroke単位の保守的な追加・削除にも対応する。`clone_vector_stroke` は選択した92-byte headerと全88-byte point recordをそのまま複製し、point位置とstroke/point bounding boxだけを整数canvas単位で平行移動して同じbodyの末尾へ追加する。brush、pressure、opacity、flag、未知fieldは保持する。

`remove_vector_stroke` は選択record以外のbyteを保持し、最後のstrokeを削除した場合は空の外部bodyにする。いずれの操作も別データとして保存されたrender cacheやpreviewを再生成しない。

```rust,no_run
# use std::fs::File;
# use clipfile::{ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let limits = Limits::default();
let database = clip.open_database()?;
let source = database
    .vector_data_sources(42, limits)?
    .into_iter()
    .next()
    .ok_or("no vector data")?;
let mut writer = clip.writer()?;
let (appended_index, _) =
    writer.clone_vector_stroke(&source, 0, 10, -5, limits)?;
writer.remove_vector_stroke(&source, appended_index, limits)?;
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Animation

`write`と`animation`を同時に有効にすると、検証済みの既存recordを対象に次を更新できる。

- `replace_animation_track_value`: `TrackValueMap`の既知型を、同じ型の有限値へ置換する。
- `replace_animation_curve_keyframe_numeric`: 既存primary FCurveの時刻・値を置換し、同名・同軸のsecondary FCurveがあれば同期する。
- `insert_animation_curve_keyframe`: 既存curveに存在する全per-key配列を、呼出側が指定した完全field値で同期延長する。
- `remove_animation_curve_keyframe`: primaryと対応secondaryの全per-key配列から同じkeyを削除する。最後のkeyは拒否する。
- `replace_animation_cel_tag`: 既存`ImageCelName` keyのTagをprimary/secondaryで同期し、現在値が同じ旧Tagを指す場合は`TrackValueMap`も同期する。
- `clone_animation_track_from_template`: 既存Trackの全非identity列とmixer本体を複製し、未追跡layerへ割り当ててtimeline末尾へ連結する。
- `clone_image_cel_track_from_template`: kind `2000`の完全なtemplateをcloneし、唯一の`ImageCelName` curveを指定した非空key列へ正規化する。
- `remove_animation_track`: timeline chainを修復してTrack rowを削除し、opaque mixer本体は保守的に保持する。

新規external objectの索引行は、既存の`ExternalChunk.ExternalID`が使うSQLite storage class（`TEXT`または`BLOB`）を保持する。CLIP STUDIO PAINTが生成した通常のファイルでは`TEXT`であり、異なる型で追加するとcontainerとSQLite自体が妥当でもアプリが読込を拒否するためである。Track複製時は`ElemScheme`が存在する場合、Track用`MaxIndex`も同じtransaction内で更新する。

curve keyの追加・削除では、既存BINC object metadataと未知fieldをそのまま保持する。存在するoptional per-key arrayはすべて値を指定して同期する必要があり、未対応fieldを持つcurveは変更前に拒否する。Tag追加等で展開後BINC長が変わる場合は、存在する`TrackActionMixerSize` / `TrackActionMixer2Size`も更新する。既存size列が外部本体と一致しない入力も変更前に拒否する。

Track cloneは任意曲線を生成するbuilderではない。`_PW_ID`はSQLiteに再採番させ、`MainId`はTrack表と、存在する場合は`ElemScheme`のTrack用`MaxIndex`の大きい方から次を割り当てる。`TrackUuid`は衝突検査済みUUID v4を生成する。primary/secondary mixerは別々の新規 `extrnlid` UUID v4へ同じ完全bodyを複製し、展開後BINC長とsize列を事前照合する。その他の未知列はSQLiteの`INSERT ... SELECT`でstorage classごと保持する。

対象timelineは`FirstTrack`から同じ`BankId`の全Trackへ一度ずつ到達して0で終端する必要がある。cloneは既存末尾の`TrackNextIndex`へ追加し、空chainなら`TimeLine.FirstTrack`を設定する。対象layerは一意な正規化可能UUIDを持ち、既存Trackから未参照でなければならない。Track kindと対象layerの意味的互換性は推測できないため、呼出側が同種のテンプレートを選ぶ。

```rust,no_run
# use std::fs::File;
use clipfile::{AnimationTrackValue, ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let mut writer = clip.writer()?;
writer.replace_animation_cel_tag(7, 0, "B", Limits::default())?;
writer.database().replace_animation_track_value(
    7,
    "Opacity",
    AnimationTrackValue::Float(75.0),
    Limits::default(),
)?;
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

```rust,no_run
# use std::fs::File;
# use clipfile::{ClipFile, Limits};
# let mut clip = ClipFile::open(File::open("input.clip")?)?;
let mut writer = clip.writer()?;
let cloned = writer.clone_animation_track_from_template(
    7,  // template Track.MainId
    1,  // target TimeLine.MainId
    42, // target untracked Layer.MainId
    Limits::default(),
)?;
println!("new track {}", cloned.track_id());
writer.write_to_path("new-output.clip")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

image-cel専用cloneも任意のmixerをゼロから作るbuilderではない。完全なkind `2000` templateを必要とし、curve metadataと非対象BINC graphはclone元を保持する。指定するkey列は非空、時刻は有限かつ昇順、tagは空でない必要がある。

Track削除はheadまたはpredecessorのlinkをtransaction内で修復する。外部mixerは他の不透明参照がないことを証明できないため、`ExternalChunk`とcontainerにorphanとして残す。`ElemScheme.MaxIndex`も安全なhigh-water markのまま減らさない。

## 現在の保証

- 元ファイルを変更しない。
- 既存出力を上書きしない。
- 変更しない外部本体とroot header後のgapをそのままコピーする。
- SQLiteの未知テーブル・未知列を保持する。
- `CHNKHead`のSQLite絶対位置と`ExternalChunk.Offset`を再計算する。
- open transaction、重複・欠落external ID、SQLite破損、上限超過を拒否する。
- block再エンコードでは展開byte数、既存block index、zlib header、再構築後のBlockData境界を検証する。
- block再エンコードの対象外payload・status・checksum・parameterを保持する。
- block再エンコード時は、同じexternal IDを参照する全`Offscreen.Attribute.BlockSize`をtransaction内で同期する。
- caller-provided stream向けの`write_to`は、書き込み前に全構造を準備・検証する。

## 現在の制約

- 対応するtop-level構成はstrict validatorが受理する既知の`CHNKHead`、`CHNKExta`、`CHNKSQLi`、`CHNKFoot`順序に限る。未知top-level chunkは失わずに書くのではなく、入力時点で拒否する。
- imageは既存base Mipmapのrender rasterまたはlayer maskの全画素置換と、plain leaf raster layerのtemplate cloneに対応する。template cloneでは派生mipmap/thumbnailを新規未索引cacheへ切り替えるが、canvas previewは再生成しない。templateなしの任意layer生成、group/mask付きlayer clone、派生cacheのpixel生成は未実装。
- textは既存objectの文字ごとの符号化幅を維持する本文置換、検証済みtemplate属性からのobject追加、1 objectを残す削除に対応する。style属性のゼロ生成、異なる文字幅へのrun再構築、座標移動、空text layerは未実装。アプリの保存済み描画cacheは本文選択などで再構築されるまで旧表示になる場合がある。
- vectorは検証済みlayoutの既存stroke平行移動、template strokeの追加、既存stroke削除、opaque body全体の置換に対応する。stroke/brushのゼロ生成、brush編集、render cache再生成は未実装。
- animationは既存Trackの保守的な完全clone、kind-2000 image-cel Trackのtemplate clone、型付き値・curve key・cel Tag更新、curve key/Track削除に対応する。templateなしの任意Track/curve metadata builder、空curve生成、mixer orphan回収は未実装。
- time-lapseと3Dのsemantic encoderは未実装。
- 再エンコードしたBlockDataは、圧縮長prefix込みのzlib payloadからCSP互換Adler-32を生成する。既存の明示的な0互換modeも後方互換のため保持する。
- 元ファイルのatomic置換APIは未実装。必要な場合も、検証済みの新規出力を利用者側で明示的に切り替える。
- CLIP STUDIO PAINTでの再オープン確認は現時点で1アプリ版である。複数版互換は継続検証する。

この制約により、現段階のwrite APIは「既存構造を保ったsemantic編集と検証済みtemplate clone、完全なopaque body差し替え、BlockData再エンコード」を対象とし、任意の`.clip`をゼロから新規生成できるとは表現しない。
