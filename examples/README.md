# Examples

このディレクトリのexampleは、`clipfile` の公開APIだけを使う小さな実行プログラムである。`inspect.rs` は横断的な診断ツール、それ以外は原則として1つの機能・目的に絞っている。

## 選び方

| example | feature | 目的 | ファイル出力 |
|---|---|---|---|
| [`inspect.rs`](inspect.rs) | なし、追加flagに応じて任意 | container、external body、任意featureをまとめて診断 | なし |
| [`inspect_document.rs`](inspect_document.rs) | `sqlite` | project、canvas、layer、layer treeを読む | なし |
| [`export_preview.rs`](export_preview.rs) | `sqlite` | 検証済みcanvas preview PNGを抽出 | 新規PNG |
| [`inspect_cmc.rs`](inspect_cmc.rs) | `sqlite` | standalone `.cmc` のpage treeと安全なpage pathを読む | なし |
| [`inspect_corrections.rs`](inspect_corrections.rs) | `sqlite` | 補正レイヤーを列挙し、型付きparameterを読む | なし |
| [`inspect_rulers.rs`](inspect_rulers.rs) | `sqlite` | 定規レイヤーと特殊定規kindを読む | なし |
| [`inspect_text.rs`](inspect_text.rs) | `sqlite` | 1レイヤーのUTF-8本文と不透明属性を読む | なし |
| [`inspect_vector.rs`](inspect_vector.rs) | `sqlite` | 1レイヤーのvector参照と上限付きraw bodyを読む | なし |
| [`export_raster.rs`](export_raster.rs) | `image` | 1レイヤーの全tileをRGBA/Gray画像へ組み立てPNGへ変換する | 新規PNG |
| [`inspect_animation.rs`](inspect_animation.rs) | `animation` | timeline、track、FCurve、cel選択を読む | なし |
| [`inspect_timelapse.rs`](inspect_timelapse.rs) | `timelapse` | manager/record/blob chainと内部frame indexをstream検証 | なし |
| [`rewrite.rs`](rewrite.rs) | `write` | 編集なしのvalidated container rewriteを行う | 新規CLIP |
| [`invert_first_tile.rs`](invert_first_tile.rs) | `write,raster` | native raster tileを1件変更し再圧縮する | 新規CLIP |
| [`invert_raster.rs`](invert_raster.rs) | `write,raster` | レイヤー画像全体をformat非依存のpixel APIで編集する | 新規CLIP |
| [`clone_raster_layer.rs`](clone_raster_layer.rs) | `write,raster` | plain raster layerをtemplateから複製し新しい全画素を設定する | 新規CLIP |
| [`edit_text.rs`](edit_text.rs) | `write` | 既存text objectを文字ごとの符号化幅を保って置換する | 新規CLIP |
| [`add_text_object.rs`](add_text_object.rs) | `write` | 既存属性templateからtext objectを追加する | 新規CLIP |
| [`remove_text_object.rs`](remove_text_object.rs) | `write` | text objectを削除し、必要なら次のobjectをprimaryへ昇格する | 新規CLIP |
| [`edit_vector_body.rs`](edit_vector_body.rs) | `write` | 検証済みvector参照のopaque body全体を置換する | 新規CLIP |
| [`translate_vector.rs`](translate_vector.rs) | `write` | 対応済みvector strokeの全pointを平行移動する | 新規CLIP |
| [`clone_vector_stroke.rs`](clone_vector_stroke.rs) | `write` | 対応済みvector strokeを複製・平行移動して末尾へ追加する | 新規CLIP |
| [`remove_vector_stroke.rs`](remove_vector_stroke.rs) | `write` | 対応済みvector strokeを1件削除する | 新規CLIP |
| [`edit_animation_cel.rs`](edit_animation_cel.rs) | `write,animation` | 既存cel keyのTagを同期更新する | 新規CLIP |
| [`clone_animation_track.rs`](clone_animation_track.rs) | `write,animation` | 既存Trackを未追跡layerへ完全複製する | 新規CLIP |
| [`insert_animation_key.rs`](insert_animation_key.rs) | `write,animation` | 既存keyをfield templateにしてFCurve keyを挿入する | 新規CLIP |
| [`remove_animation_key.rs`](remove_animation_key.rs) | `write,animation` | FCurve keyをprimary/secondaryから同期削除する | 新規CLIP |
| [`clone_image_cel_track.rs`](clone_image_cel_track.rs) | `write,animation` | kind-2000 templateから指定key列のimage-cel Trackを作る | 新規CLIP |
| [`remove_animation_track.rs`](remove_animation_track.rs) | `write,animation` | timeline chainを修復してTrack rowを削除する | 新規CLIP |

`layer-id` と `canvas-id` は [`inspect_document`](inspect_document.rs) で確認できる。

## 基本・総合診断

最初は、依存featureなしでcontainerを検証する。

```console
cargo run --example inspect -- input.clip
cargo run --example inspect -- input.clip --deep
```

`inspect` のbase / `--deep` / `--database` は将来形式の診断にも使える低レベルcontainer・SQLite APIのexampleである。`--deep` は外部本体を分類し、`BlockData` の境界と件数を調べるが、大きな圧縮payload自体は展開しない。`--document` / `--raster` / `--animation` / `--timelapse` のsemantic feature modeは型付きAPIだけを使用する。

任意featureを有効にすると、同じプログラムで横断診断できる。

```console
cargo run --all-features --example inspect -- input.clip --database --document --raster --animation --timelapse
```

個別APIを学ぶ場合は、以下の小さいexampleを優先する。

## SQLiteと文書モデル

project、canvas、layer、treeの高レベルモデル:

```console
cargo run --features sqlite --example inspect_document -- input.clip
```

canvas previewは、PNG signatureとIHDR寸法を検証してから新規ファイルへ書く。

```console
cargo run --features sqlite --example export_preview -- input.clip 1 new-preview.png
```

`.cmc` は `.clip` containerではなくstandalone SQLiteとして開く。

```console
cargo run --features sqlite --example inspect_cmc -- project.cmc
```

## レイヤー固有データ

補正レイヤーと定規は、schema差を吸収する型付きAPIで候補layerを列挙・検証する。

```console
cargo run --features sqlite --example inspect_corrections -- input.clip
cargo run --features sqlite --example inspect_rulers -- input.clip
```

`inspect_corrections` と `inspect_rulers` はそれぞれ `Database::correction_layers` / `Database::ruler_layers` を使うため、利用側でschema判定やSQLを書く必要はない。候補layerの検出からpayload・所有関係・chainの検証までライブラリ内で行う。

textとvectorは対象layer IDを指定する。text本文はUTF-8として復号されるが、style属性とvector bodyの意味はまだ不透明なため、exampleもbyte数までを安全に扱う。

```console
cargo run --features sqlite --example inspect_text -- input.clip 42
cargo run --features sqlite --example inspect_vector -- input.clip 42
```

## Raster

`export_raster` はrender rasterを解決し、全tileをrow-majorのRGBA8、Gray8、GrayAlpha8へ組み立てる。`RasterImage::into_dynamic_image` へ所有権を移し、image-rsにPNG encodingを任せる。既存出力は上書きしない。

```console
cargo run --features image --example export_raster -- input.clip 42 new-layer.png
```

レイヤーマスクへ応用する場合は、example内の `layer_raster_source` を `layer_mask_raster_source` に置き換えられる。

`image` featureを有効にすると、`RasterImage::into_dynamic_image` でpixel列を複製せずimage-rsの `DynamicImage` へ移せる。CLIP固有の `RasterDataState` は変換先に含まれないため、exampleも変換前に取得している。

## Animationとtime-lapse

`inspect_animation` は選択timeline、検証済みtrack chain、primary/secondary FCurve、現在frameのcel tagを表示する。

```console
cargo run --features animation --example inspect_animation -- input.clip
```

`inspect_timelapse` は圧縮blobを一括保持せずにstream展開し、内部 `GMIK` / `GMID` とWebP境界を検証する。表示するframe詳細は各recordの先頭3件に制限している。

```console
cargo run --features timelapse --example inspect_timelapse -- input.clip
```

## Write

`rewrite` は変更を加えずにcontainerを再構築する。対応構成ではbyte-for-byte同一になることを想定し、新規出力を再オープンしてcontainer、SQLite、external indexを検証する。

```console
cargo run --features write --example rewrite -- input.clip new-output.clip
```

`invert_first_tile` は、読み取りAPIで1つの実在tileを選び、native byteを変更して `replace_block_bytes` へ渡す例である。色編集の例ではないためalphaを含む全native byteを反転する。再圧縮時は `BlockChecksumMode::CspCompatible` で、圧縮長prefixとzlib payloadからCSP互換チェックサムを生成する。

```console
cargo run --features "write,raster" --example invert_first_tile -- input.clip new-output.clip 42
```

`invert_raster` は全tileを `RasterImage` へ組み立て、共通の `pixel_iter_mut()` と `pixel.invert()` だけでformatを分岐せず編集する。`replace_layer_raster` はformatとCSP互換checksumを画像から自動選択する。共通proxyでは `invert`、`checked_add_assign` / `checked_sub_assign`、`saturating_add_assign` / `saturating_sub_assign` を利用でき、alphaは維持される。RGBA8の `pixel.r/g/b/a`、Gray8の `pixel.value`、GrayAlpha8の `pixel.value/alpha` を直接扱う場合はformat別iteratorも利用できる。raw byteの `pixels` / `into_pixels` とchecksum指定APIは相互運用・調査用に残している。

```console
cargo run --features "write,raster" --example invert_raster -- input.clip new-output.clip 42
```

`clone_raster_layer` は同じcanvasのplain leaf raster layerをtemplateにし、完全なRGBA8/Gray8/GrayAlpha8画像を与えて親layerの先頭へ新規layerを追加する。例では入力画像を反転して新しいpixel列を作る。row identity、layer UUID、外部IDは再生成されるが、未知metadataはtemplateから保持される。100% base renderだけを外部本体として生成し、派生mipmapとthumbnailには古いtemplate cacheを参照しない新規未索引IDを割り当てる。canvas previewは再生成しない。

```console
cargo run --features "write,raster" --example clone_raster_layer -- input.clip new-output.clip 42 1 "New raster"
```

text本文はopaque属性内のrun位置を壊さないよう、対応する各文字のUTF-8 byte幅とUTF-16 code unit幅を維持する。

```console
cargo run --features write --example edit_text -- input.clip new-output.clip 42 0 Hello
```

`add_text_object` はtemplateのmain/additional style/layout byteを複製し、両方のparameter 50へ文書内で一意な同じ新IDを割り当てる。文字列・属性・primaryを含む追加属性の3配列は一度に同期される。本文はtemplateと文字ごとのUTF-8/UTF-16幅が一致する必要があり、geometryも複製されるため初期位置は重なりうる。

```console
cargo run --features write --example add_text_object -- input.clip new-output.clip 42 0 World
```

`remove_text_object` は文字列・main属性・additional属性を同期して削除する。index 0を削除すると次のobjectがprimaryへ昇格する。text layerを空にする表現は採用せず、最後の1 objectの削除は拒否する。

```console
cargo run --features write --example remove_text_object -- input.clip new-output.clip 42 1
```

vector内部のstroke serializerは未確定であるため、`edit_vector_body` は既存row IDを検証し、別途用意した完全なopaque bodyだけを差し替える。

```console
cargo run --features write --example edit_vector_body -- input.clip new-output.clip 42 7 vector-body.bin
```

検証済み92-byte stroke header / 88-byte point layoutでは、未知fieldを保持したまま位置とbounding boxを平行移動できる。別layoutは変更せず拒否する。

```console
cargo run --features write --example translate_vector -- input.clip new-output.clip 42 7 10 -5
```

同じ検証済みlayoutでは、1 strokeの完全なheader・point recordを保持したまま複製し、座標とbounding boxだけを平行移動して末尾へ追加できる。削除は選択record以外のbyteを保持し、最後のstrokeなら空のvector bodyにする。いずれも別データとして保存されたrender cacheやpreviewは再生成しないため、保存直後のキャッシュ表示とvector本体が一時的に異なる場合がある。

```console
cargo run --features write --example clone_vector_stroke -- input.clip new-output.clip 42 7 0 10 -5
cargo run --features write --example remove_vector_stroke -- input.clip new-output.clip 42 7 1
```

既存cel keyのTagはprimary/secondary curveと、同じ旧Tagを指す現在値を同期して置換する。

```console
cargo run --features "write,animation" --example edit_animation_cel -- input.clip new-output.clip 7 0 B
```

既存Trackをテンプレートとして、互換性のある未追跡layerへ複製する。`MainId`、`TrackUuid`、primary/secondary mixerのexternal IDは新規生成され、対象timelineの末尾へ連結される。

`TrackKind`とlayer種別の意味的互換性は自動判定できない。同じ用途・同じ種類の元layerを持つTrackだけをテンプレートに選び、無関係なkind同士を組み合わせないこと。

```console
cargo run --features "write,animation" --example clone_animation_track -- input.clip new-output.clip 7 1 42
```

既存FCurveへkeyを追加する場合、存在するoptional arrayをすべて同時に延長する必要がある。`insert_animation_key` は `AnimationCurveKeyframeInsert::from_template` に既存keyと新しい時刻・値を渡し、optional fieldの完全なコピーをライブラリへ任せる。軸なしcurveには `-` を渡す。削除もprimary/secondaryを同期するが、curveを空にする最後のkey削除は拒否する。

```console
cargo run --features "write,animation" --example insert_animation_key -- input.clip new-output.clip 7 Opacity - 0 1 60 75
cargo run --features "write,animation" --example remove_animation_key -- input.clip new-output.clip 7 Opacity - 1
```

image-cel専用cloneは、検証済みkind `2000` Trackをtemplateにして完全なopaque mixer graphを複製し、唯一の `ImageCelName` curveを指定した非空key列へ置換する。各keyは `<time-60hz> <cel-tag>` の順で指定する。`ImageCelTrackCloneOptions::from_timed_cels` が同一tagへ同じ内部numeric valueを割り当てるため、利用側は冗長な保存形式を管理しない。任意のcurve metadataをゼロから生成するAPIではなく、target layerはtemplateと同じ用途・種類を選ぶ。

```console
cargo run --features "write,animation" --example clone_image_cel_track -- input.clip new-output.clip 7 1 42 0 A 60 B
```

Track削除はtimelineのheadまたはpredecessor linkを修復する。opaque mixer外部本体は別参照の可能性を排除できないため、container内に保守的なorphanとして残す。

```console
cargo run --features "write,animation" --example remove_animation_track -- input.clip new-output.clip 1 7
```

write exampleはいずれも入力ファイルと既存出力を変更しない。

## API coverage

現在の主要公開領域は次のexampleで個別に確認できる。

- container / chunk / external body: `inspect`
- SQLite schema / external index: `inspect --database`
- project / canvas / layer / tree / preview: `inspect_document`, `export_preview`
- CMC / correction / ruler / text / vector: 対応する `inspect_*`
- raster assembly / native tile / layer image write: `export_raster`, `invert_first_tile`, `invert_raster`, `clone_raster_layer`
- animation / time-lapse: `inspect_animation`, `edit_animation_cel`, `clone_animation_track`, `insert_animation_key`, `remove_animation_key`, `clone_image_cel_track`, `remove_animation_track`, `inspect_timelapse`
- validated rewrite / text / vector: `rewrite`, `edit_text`, `add_text_object`, `remove_text_object`, `edit_vector_body`, `translate_vector`, `clone_vector_stroke`, `remove_vector_stroke`

通常のsemantic exampleは公開APIが保証する境界を越えず、未解明のstyle、templateなしのvector brush/animation metadata、time-lapse、3Dデータを推測して生成しない。低レベルescape hatchの範囲と監査結果は [`../docs/api-level-audit.md`](../docs/api-level-audit.md) にまとめている。
