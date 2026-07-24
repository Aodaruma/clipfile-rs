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
| [`export_raster.rs`](export_raster.rs) | `raster` | 1レイヤーの全tileをRGBA/Gray画像へ組み立てる | 新規PAM |
| [`inspect_animation.rs`](inspect_animation.rs) | `animation` | timeline、track、FCurve、cel選択を読む | なし |
| [`inspect_timelapse.rs`](inspect_timelapse.rs) | `timelapse` | manager/record/blob chainと内部frame indexをstream検証 | なし |
| [`rewrite.rs`](rewrite.rs) | `write` | 編集なしのvalidated container rewriteを行う | 新規CLIP |
| [`invert_first_tile.rs`](invert_first_tile.rs) | `write,raster` | native raster tileを1件変更し再圧縮する | 新規CLIP |
| [`invert_raster.rs`](invert_raster.rs) | `write,raster` | レイヤー画像全体をRGBA8で編集する | 新規CLIP |
| [`edit_text.rs`](edit_text.rs) | `write` | 既存text objectを文字ごとの符号化幅を保って置換する | 新規CLIP |
| [`add_text_object.rs`](add_text_object.rs) | `write` | 既存属性templateからtext objectを追加する | 新規CLIP |
| [`edit_vector_body.rs`](edit_vector_body.rs) | `write` | 検証済みvector参照のopaque body全体を置換する | 新規CLIP |
| [`translate_vector.rs`](translate_vector.rs) | `write` | 対応済みvector strokeの全pointを平行移動する | 新規CLIP |
| [`edit_animation_cel.rs`](edit_animation_cel.rs) | `write,animation` | 既存cel keyのTagを同期更新する | 新規CLIP |
| [`clone_animation_track.rs`](clone_animation_track.rs) | `write,animation` | 既存Trackを未追跡layerへ完全複製する | 新規CLIP |

`layer-id` と `canvas-id` は [`inspect_document`](inspect_document.rs) で確認できる。

## 基本・総合診断

最初は、依存featureなしでcontainerを検証する。

```console
cargo run --example inspect -- input.clip
cargo run --example inspect -- input.clip --deep
```

`--deep` は外部本体を分類し、`BlockData` の境界と件数を調べる。大きな圧縮payload自体は展開しない。

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

補正レイヤーと定規は候補layerをDBから列挙し、型付きAPIで検証する。

```console
cargo run --features sqlite --example inspect_corrections -- input.clip
cargo run --features sqlite --example inspect_rulers -- input.clip
```

textとvectorは対象layer IDを指定する。text本文はUTF-8として復号されるが、style属性とvector bodyの意味はまだ不透明なため、exampleもbyte数までを安全に扱う。

```console
cargo run --features sqlite --example inspect_text -- input.clip 42
cargo run --features sqlite --example inspect_vector -- input.clip 42
```

## Raster

`export_raster` はrender rasterを解決し、全tileをrow-majorのRGBA8またはGray8へ組み立てる。追加の画像crateを必要としないよう、出力はNetpbm PAM形式である。既存出力は上書きしない。

```console
cargo run --features raster --example export_raster -- input.clip 42 new-layer.pam
```

レイヤーマスクへ応用する場合は、example内の `layer_raster_source` を `layer_mask_raster_source` に置き換えられる。

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

text本文はopaque属性内のrun位置を壊さないよう、対応する各文字のUTF-8 byte幅とUTF-16 code unit幅を維持する。

```console
cargo run --features write --example edit_text -- input.clip new-output.clip 42 0 Hello
```

`add_text_object` はtemplateのmain/additional style/layout byteを複製し、両方のparameter 50へ文書内で一意な同じ新IDを割り当てる。文字列・属性・primaryを含む追加属性の3配列は一度に同期される。本文はtemplateと文字ごとのUTF-8/UTF-16幅が一致する必要があり、geometryも複製されるため初期位置は重なりうる。

```console
cargo run --features write --example add_text_object -- input.clip new-output.clip 42 0 World
```

vector内部のstroke serializerは未確定であるため、`edit_vector_body` は既存row IDを検証し、別途用意した完全なopaque bodyだけを差し替える。

```console
cargo run --features write --example edit_vector_body -- input.clip new-output.clip 42 7 vector-body.bin
```

検証済み92-byte stroke header / 88-byte point layoutでは、未知fieldを保持したまま位置とbounding boxを平行移動できる。別layoutは変更せず拒否する。

```console
cargo run --features write --example translate_vector -- input.clip new-output.clip 42 7 10 -5
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

write exampleはいずれも入力ファイルと既存出力を変更しない。

## API coverage

現在の主要公開領域は次のexampleで個別に確認できる。

- container / chunk / external body: `inspect`
- SQLite schema / external index: `inspect --database`
- project / canvas / layer / tree / preview: `inspect_document`, `export_preview`
- CMC / correction / ruler / text / vector: 対応する `inspect_*`
- raster assembly / native tile / layer image write: `export_raster`, `invert_first_tile`, `invert_raster`
- animation / time-lapse: `inspect_animation`, `edit_animation_cel`, `clone_animation_track`, `inspect_timelapse`
- validated rewrite / text / vector: `rewrite`, `edit_text`, `add_text_object`, `edit_vector_body`, `translate_vector`

各exampleは公開APIが保証する境界を越えず、未解明のstyle、vector stroke、time-lapse、3Dデータを推測して生成しない。
