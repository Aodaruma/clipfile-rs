# 1.0公開前チェックリスト

この文書は、`clipfile` 1.0で保証する範囲と公開直前の確認項目を固定する。独自形式を完全に新規生成することではなく、検証済み構造を安全に読み、既存構造または互換templateを保った編集を行えることをstable APIの境界とする。

## 1.0の対象範囲

- container、SQLite index、document/layer treeの上限付き読み取り
- RGBA8/Gray8 rasterの復号・全画素置換とplain raster layerのtemplate clone
- text本文の保守的置換、template-based object追加、1 objectを残す削除
- 検証済みvector layoutの平行移動、stroke clone、stroke削除
- animation timeline/track/curve読み取り、既存値・key・cel tag編集
- 完全なTrack template clone、kind-2000 image-cel Track clone、Track unlink
- CSP互換BlockData checksum、external object追加、offset修復、新規出力の再オープン検証

templateなしのstyle/brush/animation metadata生成、派生raster cacheとcanvas previewの生成、time-lapse/3Dのsemantic writeは1.0の保証範囲外とする。未対応形式を推測して書く代わりに明示的なエラーを返す。

## 公開直前ゲート

- [x] `cargo fmt --check`
- [x] `cargo clippy --all-targets --all-features -- -D warnings`
- [x] `cargo test --all-features`
- [x] `cargo test --doc --all-features`
- [x] `cargo check --examples --all-features`
- [x] 宣言MSRVとLinux/Windows/macOS stable CI
- [x] `cargo package` とpackage内容の機密・ローカルfixture混入確認
- [x] 0.5系に対する公開API差分とSemVer方針の最終確認
- [x] 新規raster/vector/text/animation出力のアプリ再読込確認
- [x] README、writing guide、examples guide、CHANGELOGのAPI名と制約の一致
- [ ] release version、tag、crates.io dry-run、GitHub Release内容の最終確認

検証用ファイル、生成script、画面出力はGit管理対象外だけに置く。公開文書には作品名、レイヤー名、本文、UUID、ユーザーパスを記録しない。

## 1.0後へ明示的に残す領域

- vector brush/style fieldとtext font/paragraph/geometryの完全なsemantic model
- template-free raster/vector/text/animation object builder
- vector render cache、派生mipmap、thumbnail、canvas previewの再生成
- animationの空curve表現と、不要と証明できるmixer orphanの回収
- time-lapse/3Dのsemantic encoder
- 未知top-level chunkを含むcontainerの損失なしrewrite
- 複数アプリ版・将来schemaに対する互換性コーパスの拡充

これらは1.0 APIの保守的な境界を崩さず、後方互換な追加機能として扱う。
