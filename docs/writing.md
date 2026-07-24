# `.clip` の書き込み

`write` featureは、既存の`.clip`を検証してから別の新規ファイルへ再構築する低レベルAPIを提供する。既定featureには含まれない。

```toml
[dependencies]
clipfile = { version = "0.3", features = ["write"] }
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

## 既存ラスターブロックの再エンコード

`replace_block_bytes` は、既存の `BlockData` 外部本体に含まれる1ブロックを、展開済みnative byte列からzlib再圧縮する。byte数は既存parameterの `channels × width × height` と完全一致する必要がある。対象以外の圧縮payload、ブロックindex・parameter・status・checksumは保持し、空ブロックへ初めてpayloadを設定することもできる。

`BlockCheckSum` のアルゴリズムは未解明である。再エンコード時は、ローカル検証したアプリ版で受理された0を保存する `BlockChecksumMode::Zero` を呼び出し側が明示しなければならない。複数アプリ版での互換性は保証しない。

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
    BlockChecksumMode::Zero,
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

## 現在の保証

- 元ファイルを変更しない。
- 既存出力を上書きしない。
- 変更しない外部本体とroot header後のgapをそのままコピーする。
- SQLiteの未知テーブル・未知列を保持する。
- `CHNKHead`のSQLite絶対位置と`ExternalChunk.Offset`を再計算する。
- open transaction、重複・欠落external ID、SQLite破損、上限超過を拒否する。
- block再エンコードでは展開byte数、既存block index、zlib header、再構築後のBlockData境界を検証する。
- block再エンコードの対象外payload・status・checksum・parameterを保持する。
- caller-provided stream向けの`write_to`は、書き込み前に全構造を準備・検証する。

## 現在の制約

- 対応するtop-level構成はstrict validatorが受理する既知の`CHNKHead`、`CHNKExta`、`CHNKSQLi`、`CHNKFoot`順序に限る。未知top-level chunkは失わずに書くのではなく、入力時点で拒否する。
- rasterは既存BlockDataのnative tile 1件を置き換える低レベルAPIだけを実装している。画像全体、vector、text、animation、time-lapse、3Dのsemantic encoderは未実装。
- `BlockCheckSum`の生成規則は未解明であり、再エンコードは明示的な0互換modeに限定する。
- 元ファイルのatomic置換APIは未実装。必要な場合も、検証済みの新規出力を利用者側で明示的に切り替える。
- CLIP STUDIO PAINTでの再オープン確認は現時点で1アプリ版である。複数版互換は継続検証する。

この制約により、現段階のwrite APIは「既存構造を保ったmetadata編集、完全なopaque body差し替え、既存BlockData内の限定的なtile再エンコード」を対象とし、任意の`.clip`を新規生成できるとは表現しない。
