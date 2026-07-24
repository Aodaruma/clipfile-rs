# 公開APIレベル監査

最終監査日: 2026-07-25

## 方針

既知の意味構造は、SQLite列名、SQL、byte order、配列添字、内部ID連鎖を利用側へ出さず、型付きAPIから取得・編集できることを原則とする。一方、CLIP STUDIO PAINTの将来versionや未知データを調査・保持できるよう、次のescape hatchは削除しない。

- `Database::schema()` / `Database::connection()` によるread-only SQLite access
- `ChunkHeader`、`ExternalObject`、`BlockData`によるcontainer / external body access
- `raw()`、raw attribute、opaque bodyによる未知値の保持
- `EditableDatabase::connection_mut()`、external body / block replacementによる高度なwrite

高級APIは既知値だけを閉じたenumへ変換せず、未知値をraw表現へ戻せるようにする。これにより、新しい値が追加されたファイルも既知部分を読みながら診断できる。

## 監査結果

| 領域 | 通常利用の高級API | 低レベルaccessの用途 |
|---|---|---|
| container | `ClipFile::open` / `validate` | 未知chunk、external bodyの調査 |
| document | `read_document` / `Document` / `LayerTree` | 未知SQLite列の調査 |
| preview | `Database::canvas_preview` | 検証済みencoded PNGの取り出し |
| correction | `correction_layers` / `correction_layer` | 未知correction payloadの保持 |
| ruler | `ruler_layers` / `ruler_layer` | 未解釈guide・curve headerの保持 |
| raster | `layer_raster_source` / `decode_raster` / `RasterImage::from_pixels` / image-rs変換 | native tile、未知packingの調査 |
| raster write | `replace_layer_raster` / `replace_layer_mask` / template clone | checksum modeやraw row-major pixelの明示指定 |
| text | `text_layer`とtyped text write | 未解釈style / layout attributeの保持 |
| vector | `vector_data_sources`と検証済みstroke操作 | 未解釈brush / stroke layoutの調査・body保持 |
| animation | `read_animation` / `read_animation_for_timeline`とtyped write | 未知Track kind / value type / mixer metadataの保持 |
| time-lapse | `time_lapse` / `read_time_lapse_frame_index` | 未解釈header word / FourCCの保持 |
| `.cmc` | `CmcFile` / page tree / `page_path` | 未知link formのraw保持 |

監査で見つかったSQL依存の補正レイヤー列挙とラスター候補探索はライブラリ側へ移した。animation keyのoptional fieldコピー、image-celの内部numeric value採番、semantic raster writeのchecksum選択も高級APIへ移した。利用側で組み立てたrasterは `RasterImage::from_pixels`、image-rsからは `try_from_dynamic_image` で寸法・formatを検証して取り込める。

現時点で、意味が解明済みなのにSQLまたはbinary parsingでしか到達できない領域はない。複数layerを対象にするtext、vector、camera、rasterは、`Document::layers()`で所有layerを列挙して各typed accessorへ渡せる。

## 意図的に低レベルなexample

- `inspect.rs`のbase / `--deep` / `--database`: container、external body、SQLite構造の診断
- `invert_first_tile.rs`: native planar tileとblock writerの説明
- `inspect_vector.rs` / `edit_vector_body.rs`: 未解明vector bodyの上限付きread / 完全置換
- `export_preview.rs`: 検証済みencoded previewをbyte変更せず抽出

これらはsemantic編集の推奨経路ではない。各ファイルと`examples/README.md`で目的を明記する。

## 新形式追加時の確認

1. raw値と未知payloadを失わず読み取れるか確認する。
2. 差分コーパスで意味が確定したら、型付きgetter / operationを追加する。
3. 通常exampleからSQL、schema分岐、binary parsing、magic numberを除去する。
4. low-level APIは互換調査用として維持し、高級APIの内部実装へ固定しない。
5. `rg`でexamples内の `connection(`、`prepare(`、`SELECT`、byte添字、endianness変換を再監査する。

未解明領域の詳細は`open-questions.md`を正とする。
