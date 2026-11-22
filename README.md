# CatSystem2 Emulator

CatSystem2 模拟器包含 Rust 引擎、Windows 播放器和 Android 应用。播放器直接读取所选目录内的游戏资源和 INT 归档。Android APK 不内置游戏。

## 目录说明

| 目录 | 职责 |
| --- | --- |
| `simulator/crates/formats/` | 游戏配置、归档、图像、音频和存档格式 |
| `simulator/crates/script/` | 脚本解码、表达式和场景指令 |
| `simulator/crates/runtime/` | 虚拟机、资源路由、任务和运行状态 |
| `simulator/crates/renderer/` | 画布、图层合成、文字、图像和粒子 |
| `simulator/crates/desktop/` | 播放会话、兼容实现、Windows 宿主和剧情回放器 |
| `simulator/crates/cli/` | Windows 播放器和命令行诊断 |
| `simulator/crates/android/` | Android C ABI、会话、画面和输入桥接 |
| `simulator/android/` | Android 应用、游戏库、Kotlin 界面、Java 播放宿主和 JNI |
| `simulator/vendor/` | 绑定到 Cargo 的第三方依赖及许可证，见 [THIRD_PARTY.md](simulator/THIRD_PARTY.md) |
| `scripts/build/` | 模拟器构建、Android 安装和媒体依赖准备 |
| `scripts/convert/` | 游戏资源及汉化补丁转换 |
| `scripts/analysis/` | PC 回放及回归用例、Android 日志与性能分析、引擎静态检查 |
| `games/`、`localization/` | 本地游戏原目录和翻译资料，Git 忽略 |
| `localization/kanmi/` | 《尚有佳蜜伴恋心》的汉化归档和加载配置，Git 忽略 |
| `reports/`、`resources/` | 本地批测清单、研究报告和参考资料，Git 忽略 |
| `archive/YYYY-MM/` | 本地测试输出和按日期归档的游戏转换、历史探针、研究资料与旧发布工具，Git 忽略 |

## 测试原游戏目录

Windows 使用已有的 `cs2-player.exe` 打开游戏目录。手动测试可使用 `window`。批量检查和短回放使用同一模拟器已有的 `story_replay.exe`，回放器把配置和存档写到隔离目录，日志及截图与测试回执一起归档。日常游戏测试复用已有播放器，无需逐游戏或逐场景编译。

```powershell
$game = (Resolve-Path -LiteralPath 'games/ISLAND').Path
& $player window $game
python -B scripts/analysis/run_playthrough_batch.py --player $replay --game-root $game --frames 12
```

`$player` 和 `$replay` 分别是现有 Windows 播放器和回放器的路径。`--game-root` 接受现有游戏目录，单个回放的用例编号为 0。多游戏回归使用 `--inventory`、`--indices` 和 `--scenarios`。批测默认读取本地启动清单，单目录回放直接接收路径。`--player` 必须指向已有回放器。

Android 使用已安装的模拟器，从游戏库选中平板上已导入的原游戏目录。需要采集真机日志或测试剧情时，使用 `scripts/analysis/android_story_replay.py` 或 `android_story_drive.py`，传入当前设备、游戏目录和已安装 APK 的身份。

## 构建模拟器

引擎或宿主改动需要生成新版时，Windows 使用 `python -B scripts/build/cs2_workspace.py build --integration`。Android 使用 `python -B scripts/build/android_workspace.py build --integration`。测试时可统一复用生成的版本。`CS2_BUILD_CACHE_ROOT` 可设置为绝对路径，集中保存构建缓存。

## 补丁配置

在汉化版游戏根目录放置 UTF-8 的 `cs2-localization.json`。补丁按列表顺序优先查找，未覆盖的内容继续走原有 `updateNN.int`、松散文件和基础归档查找顺序。可选的 `scene_encoding` 支持 `cp932`、`gbk` 和 `utf8`，只作用于从该补丁读出的 CST。

```json
{
  "patches": [
    { "archive": "translation.int", "scene_encoding": "gbk" }
  ]
}
```

Android 游戏库可长按游戏选择「语种」，选项在下次启动时生效。「自动」沿用游戏和补丁默认设置。「日语原文」选择游戏的日语语言表并跳过可分离的汉化包和替字字体。「本地汉化」优先使用游戏自带的中文语言表和已接入的本地汉化资源。若补丁直接替换过原始资源，日语需要同版本的原始数据。

### 尚有佳蜜伴恋心

使用现有汉化补丁生成播放器可加载的 `kanmi_cn.int` 和 `cs2-localization.json`。`$launcher` 指向汉化补丁自带的启动器，`$scene` 指向原版 `scene.int`，`$images` 指向补丁的 `image` 目录。

```powershell
python -B -X utf8 scripts/convert/restore_kanmi_localization.py $launcher $scene $images localization/kanmi
```

将生成的两个文件放到与原版 `scene.int` 同一目录。播放器会按 GBK 加载补丁里的对白和说话人姓名。
