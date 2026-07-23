# 開発・公開手順

## 現在の構成

- crate名: `clipfile`
- version: `0.2.0`
- edition: Rust 2024
- MSRV: Rust 1.85
- license: MIT
- 既定featureの依存: なし
- `sqlite` feature: `rusqlite` と同梱SQLite（システムのSQLite開発環境は不要）
- `raster` feature: `sqlite` + pure-Rust zlib展開
- `animation` feature: `sqlite` + BINCタイムライン曲線
- `timelapse` feature: `sqlite` + zlibタイムラプスBLOB

CIはLinux、Windows、macOSのstableとLinuxのMSRVでテストし、fmt、Clippy、rustdoc、`cargo package` も検証する。DependabotはCargo依存とGitHub Actionsを週次確認する。

## ローカル検証

```console
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features --locked
cargo doc --all-features --no-deps
cargo package --locked
```

ローカルサンプルを検証する場合:

```console
cargo run --example inspect -- tester/data/example.clip
cargo run --features sqlite --example inspect -- tester/data/example.clip --database
cargo run --features sqlite --example inspect -- tester/data/example.clip --document
cargo run --features raster --example inspect -- tester/data/example.clip --raster
```

`tester/` はGit管理対象外である。サンプル、抽出DB、参考リポジトリ、解析結果を置けるが、「ignoreされている」ことは再配布許可を意味しない。機密作品や第三者作品をissueへ添付しない。

実ファイル検証では次の境界を守る。

- `.clip`、抽出DB、画像出力、検証専用スクリプトは `tester/` の外へ出さない。
- commit前に `git status --ignored --short tester/` と `cargo package --list` で混入がないことを確認する。
- 文書化するのは匿名IDまたは集計値、形式上必要な構造・数値だけとする。
- 元ファイル名、作品名、レイヤー名、ユーザー名、絶対パス、画面に表示された最近使った場所をdocs・テスト名・commit messageへ記録しない。

## crates.io公開

公開リポジトリは `https://github.com/Aodaruma/clipfile-rs` としてmanifestへ設定済みである。

2026-07-22時点のcrates.io indexでは同名crateを確認できなかったが、crate名は予約されていない。初回公開直前に再確認する。

crates.ioのTrusted Publishingは初回リリースには利用できないため、`0.1.0`だけはmaintainerがローカルから公開する。

1. crate名とmanifest内の公開情報が最新であることを確認する。
2. `CHANGELOG.md` の `[Unreleased]` を対象versionへ移す。
3. `cargo package --list` で、サンプルや `tester/` が含まれないことを確認する。
4. 上記のローカル検証とCIをすべて通す。
5. `cargo publish --dry-run --locked` を実行する。
6. crates.ioへGitHubアカウントでログインし、メールアドレスを検証する。
7. crates.ioで発行したAPI tokenを`cargo login`へ入力する。tokenをリポジトリやGitHub Actions secretへ保存しない。
8. `cargo publish --locked` を実行する。この操作は取り消せず、同じversionを上書きできない。
9. crates.ioのcrate設定でTrusted Publisherを次のように登録する。
   - GitHub owner: `Aodaruma`
   - repository: `clipfile-rs`
   - workflow: `publish.yml`
   - environment: `release`
10. 公開したcommitへ `v0.1.0` のような署名付きtagを作成してpushする。

tagのversionは`Cargo.toml`と一致しなければならない。`.github/workflows/publish.yml`は、未公開versionをOIDCの短期tokenでcrates.ioへ公開し、GitHub Releaseを作成する。すでに手動公開済みのversionは再公開せず、GitHub Release作成だけを行う。

crates.ioへ公開したversionは上書き・削除できない。問題があるversionはyankできるが、内容自体は残る。GitHubの`release` environmentには必要に応じて承認者やtag保護規則を設定する。

公式手順:

- [Publishing on crates.io](https://doc.rust-lang.org/cargo/reference/publishing.html)
- [`cargo publish`](https://doc.rust-lang.org/cargo/commands/cargo-publish.html)
- [Trusted Publishing](https://crates.io/docs/trusted-publishing)
- [The `rust-version` field](https://doc.rust-lang.org/cargo/reference/rust-version.html)

## バージョン方針

- 破壊的な公開API変更は、`0.x` の間もCHANGELOGへ明記する。
- MSRV変更はminor releaseで行い、CIのMSRV matrixと同時に更新する。
- ファイル形式の新しい推定を「対応」と呼ぶ前に、合成テストと複数の実ファイルで確認する。
- 書き込みAPIは、未知データを保全できるまで既定featureへ入れない。
