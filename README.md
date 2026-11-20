# CatSystem2-emulator

CatSystem2 引擎开源实现。An independent, open-source implementation of the CatSystem2 engine in Rust.

项目实现游戏资源读取、脚本执行、图层渲染、文字与音视频，并提供 Windows 和 Android 宿主。兼容性仍在开发和验证中；能进入标题或正文不代表已经通关。

本项目不隶属于 CatSystem2 原厂。不提供商业游戏、原版运行库、游戏资源、存档或翻译补丁。运行时请使用你自己的游戏文件。

## 目录说明

- `simulator/crates/`：资源格式、脚本、引擎运行时、渲染、桌面宿主和 Android 接口。
- `simulator/crates/desktop/examples/`：无窗口剧情回放示例，可按帧发送输入和保存截图。
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

Android 集成构建使用 Windows、Python 3.12、完整的 JDK、Android SDK 37、NDK 28.2.13676358、CMake 3.22.1、MSYS2 的 `bash` 和 `make`、原生 GCC，以及 Rust `aarch64-linux-android` 目标。将 `JAVA_HOME` 指向完整的 JDK 21，或 Android Studio 自带的 JBR。该目录的 `bin` 必须包含 `java.exe`、`javac.exe` 和 `jlink.exe`。构建使用仓库内的 Gradle 9.4.1 Wrapper，可自动下载 Gradle；首次构建也会下载并校验固定版本的 FFmpeg 源码。

SDK 位置依次取自 `--sdk`、`ANDROID_HOME`、`ANDROID_SDK_ROOT`，默认使用当前用户的 `AppData/Local/Android/Sdk`。MSYS2 可通过 `PATH` 查找，默认位置为 `C:/msys64`。安装在其他位置时，可设置 `CS2_MSYS2_ROOT` 为安装目录。GCC 可通过 `PATH` 或 MSYS2 的 `ucrt64`、`mingw64` 工具链查找，也可以通过 `CS2_HOST_CC` 指定编译器的绝对路径。请将 Android SDK、MSYS2 和 C 编译器安装在纯 ASCII 路径下。

在仓库根目录运行以下命令。

```powershell
rustup target add aarch64-linux-android
python -B -X utf8 scripts/build/android_workspace.py build --integration
```

开发集成构建的 APK、媒体源代码包及构建日志保存在本地 `archive/`。如需从 Android Studio 运行，先完成上述构建，再打开 `simulator/android`。在项目的 `local.properties` 中，把 `cs2.nativeBuildDir` 设置为本次 `build.json` 里 `android_paths.staging` 的绝对路径。Windows 路径可使用正斜杠，例如 `cs2.nativeBuildDir=D:/path/to/attempt`。该目录含有 Studio 需要的 `rust-libs`、`media-libs` 和 `media-notices`。

加载与翻页截图探针另外需要 Pillow。请勿在公开问题中上传游戏文件、存档或含设备标识的未脱敏日志。

## 许可

项目源码默认采用 [MIT](LICENSE)；渲染 crate 按其 [LGPL-2.1-only](simulator/crates/renderer/LICENSE-LGPL-2.1) 许可发布。第三方依赖保留各自的许可证，见 [第三方说明](simulator/THIRD_PARTY.md)。
