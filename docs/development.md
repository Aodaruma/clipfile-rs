# 開発・公開手順

## 現在の構成

- crate名: `clipfile`
- version: `0.1.0`
- edition: Rust 2024
- MSRV: Rust 1.85
- license: MIT
- 通常依存: なし

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
```

`tester/` はGit管理対象外である。サンプル、抽出DB、参考リポジトリ、解析結果を置けるが、「ignoreされている」ことは再配布許可を意味しない。機密作品や第三者作品をissueへ添付しない。

## 初回公開前に必ず行うこと

公開リポジトリは `https://github.com/Aodaruma/clipfile-rs` としてmanifestへ設定済みである。

2026-07-21時点の `cargo search clipfile` では同名crateを確認できなかったが、crate名は予約されていない。公開直前に再確認する。

1. crate名とmanifest内の公開情報が最新であることを確認する。
2. `CHANGELOG.md` の `[Unreleased]` を対象versionへ移す。
3. `cargo package --list` で、サンプルや `tester/` が含まれないことを確認する。
4. 上記のローカル検証とCIをすべて通す。
5. `cargo publish --dry-run --locked` を実行する。
6. crates.ioのアカウント・トークンを準備し、`cargo publish --locked` を実行する。
7. 公開したcommitへ `v0.1.0` のような署名付きtagを作り、GitHub Releaseを作成する。

crates.ioへ公開したversionは上書き・削除できない。問題があるversionはyankできるが、内容自体は残る。公開コマンドは自動CIに入れず、当面はmaintainerがパッケージ内容を確認して実行する。

公式手順:

- [Publishing on crates.io](https://doc.rust-lang.org/cargo/reference/publishing.html)
- [`cargo publish`](https://doc.rust-lang.org/cargo/commands/cargo-publish.html)
- [The `rust-version` field](https://doc.rust-lang.org/cargo/reference/rust-version.html)

## バージョン方針

- 破壊的な公開API変更は、`0.x` の間もCHANGELOGへ明記する。
- MSRV変更はminor releaseで行い、CIのMSRV matrixと同時に更新する。
- ファイル形式の新しい推定を「対応」と呼ぶ前に、合成テストと複数の実ファイルで確認する。
- 書き込みAPIは、未知データを保全できるまで既定featureへ入れない。
