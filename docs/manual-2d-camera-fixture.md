# 2Dカメラ最小差分の手動作成手順

この手順は、Computer Useで安定して操作できなかった階層メニューを人手で1回だけ操作し、2DカメラのSQLite・action mixer差分を取得するためのものです。今すぐ実行する必要はありません。

## 保存先と匿名性

- 新規の匿名アニメーション文書だけを使う。
- 保存先はGit管理対象外の `tester/data/generated/` とする。
- 作品名、人物名、実案件名、本文、既存素材は入れない。
- 次の3ファイル以外は作らなくてよい。

  - `camera_manual_baseline.clip`
  - `camera_manual_folder.clip`
  - `camera_manual_keyframe.clip`

## 手順

1. CLIP STUDIO PAINT EXで小さな新規アニメーション文書を作る。既定のアニメーションフォルダーとタイムラインは残す。フレームレート・再生時間・出力サイズは任意だが、途中で変更しない。
2. 何も追加せず、`camera_manual_baseline.clip` として保存する。
3. 公式手順どおり、`アニメーション` → `アニメーション用新規レイヤー` → `2Dカメラフォルダー` を選ぶ。
4. ダイアログでは名前を `CameraTest` とし、出力サイズは現在値のまま `OK` を押す。
5. 既定のアニメーションフォルダーを `CameraTest` の中へ移動し、`camera_manual_folder.clip` として別名保存する。
6. タイムライン上で `CameraTest` を選び、先頭以外のフレームへ移動する。
7. オブジェクトツールでカメラ枠を少し右へ動かし、そのフレームに変形キーが表示されたことを確認する。回転・拡大率・不透明度は変更しない。
8. `camera_manual_keyframe.clip` として別名保存する。

2Dカメラフォルダーは常にキーフレーム編集が有効で、作成コマンドは `Animation > New animation layer > 2D Camera Folder` にあることを[公式マニュアル](https://help.clip-studio.com/en-us/manual_en/600_animation/Track_operations.htm)で確認済みです。

## 完了後

3ファイルが揃った時点で知らせてください。解析側ではファイル名・レイヤー名・ユーザーパスを公開文書へ転記せず、列・track kind・curve名・payload構造の差だけを集計します。
