# CatSystem2-emulator

CatSystem2 引擎开源实现。An independent, open-source implementation of the CatSystem2 engine in Rust.

项目实现游戏资源读取、脚本执行、图层渲染、文字与音视频，并提供 Windows 和 Android 宿主。兼容性仍在开发和验证中；能进入标题或正文不代表已经通关。

本项目不隶属于 CatSystem2 原厂。不提供商业游戏、原版运行库、游戏资源、存档或翻译补丁。运行时请使用你自己的游戏文件。

## 目录说明

- `simulator/crates/`：资源格式、脚本、引擎运行时、渲染、桌面宿主和 Android 接口。
- `simulator/android/`：Android 工程及 JNI/Surface 集成。
- `simulator/vendor/`：固定版本的第三方依赖及对应许可证。
- `scripts/build/`：Android 构建和固定版本 FFmpeg 构建工具。
- `scripts/analysis/`：加载、Surface 帧间隔、运行阶段与翻页测量工具。

## 构建

在仓库根目录使用支持 Rust 2024 的工具链。Windows GNU 工具链还需要 `windres`；如果不在 `PATH`，可通过 `WINDRES` 指定。使用 GNU 工具链时，请将签出和 `CARGO_TARGET_DIR` 放在纯 ASCII 路径下，避免链接器无法读取含非 ASCII 字符的文件路径。

```powershell
cargo build --locked --manifest-path simulator/Cargo.toml -p cs2-cli
```

将你自己的游戏目录作为参数传入构建生成的 Windows 命令行程序。部分集成测试需要本仓库不提供的外部游戏样本。

Android 集成构建目前使用 Windows、Python 3、JDK 17+、Gradle 9.5.1、Android SDK 37、NDK 28.2.13676358、CMake 3.22.1、MSYS2 和 Rust `aarch64-linux-android` 目标。FFmpeg 构建工具会校验上游源码的哈希。使用 `--sdk` 可指定 SDK 位置。

```powershell
python -B -X utf8 scripts/build/android_workspace.py build --integration
```

这是开发集成构建，不是通过全游戏验收的发行版。生成的 APK、媒体源代码包及构建日志保存在本地 `archive/`。

加载与翻页截图探针另外需要 Pillow。请勿在公开问题中上传游戏文件、存档或含设备标识的未脱敏日志。

## 许可

项目源码默认采用 [MIT](LICENSE)；渲染 crate 按其 [LGPL-2.1-only](simulator/crates/renderer/LICENSE-LGPL-2.1) 许可发布。第三方依赖保留各自的许可证，见 [第三方说明](simulator/THIRD_PARTY.md)。
