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

`invert_first_tile` は、読み取りAPIで1つの実在tileを選び、native byteを変更して `replace_block_bytes` へ渡す例である。色編集の例ではないためalphaを含む全native byteを反転する。再圧縮時の `BlockCheckSum` は未解明であり、`BlockChecksumMode::Zero` を明示する。複数アプリ版での互換性は保証していない。

```console
cargo run --features "write,raster" --example invert_first_tile -- input.clip new-output.clip 42
```

どちらのwrite exampleも入力ファイルと既存出力を変更しない。

## API coverage

現在の主要公開領域は次のexampleで個別に確認できる。

- container / chunk / external body: `inspect`
- SQLite schema / external index: `inspect --database`
- project / canvas / layer / tree / preview: `inspect_document`, `export_preview`
- CMC / correction / ruler / text / vector: 対応する `inspect_*`
- raster assembly / native tile: `export_raster`, `invert_first_tile`
- animation / time-lapse: `inspect_animation`, `inspect_timelapse`
- validated rewrite / BlockData再圧縮: `rewrite`, `invert_first_tile`

未知形式を推測して生成するsemantic encoderはまだ提供していない。各exampleも公開APIが保証する境界を越えず、raw値や不透明byteを勝手に解釈しない。
