# CatSystem2 Emulator

基于 Rust 重新实现的开源 **CatSystem2** 视觉小说引擎，支持 **Windows** 与 **Android** 平台。

只需要自备游戏本体目录，播放器即可直接读取游戏内的脚本、图片、音视频与 `.int` 封包文件并运行。**本仓库与安装包均不内置任何游戏资源。**

## 主要特性

- **原版引擎还原**：支持常见的 CatSystem2 剧本（`.cst` / `.fes` / `.kcs`）、图像与动画（HG3、ANM、粒子特效、E-mote 动态立绘）、音频与视频播放（Android 端集成 FFmpeg 解码），以及游戏内设置、历史记录、鉴赏和存读档功能。
- **灵活的汉化与语言切换**：
  - 在游戏根目录放入 `cs2-localization.json`，即可按顺序优先加载外挂汉化封包（支持 `cp932` / `gbk` / `utf8` 编码），无需替换原版游戏文件。
  - Android 端在游戏库长按游戏即可切换语种：**自动**、**日语原文** 或 **本地汉化**。
  - 内置《尚有佳蜜伴恋心》与《ツナバン らぶみくす》等作品的汉化包转换工具（位于 `scripts/convert/`）。
- **移动端操作与调试工具**：
  - Android 端支持按原比例显示画面、触摸操作、外接键鼠与软键盘，并可通过侧边「游戏控制」面板快速执行右键返回、Esc、Enter、长按 Ctrl 快进、Page Up / Down 翻页及打开游戏内菜单。
  - 支持开启实时性能监控浮窗（显示 FPS、GPU、CPU、内存与功耗）。

## 如何使用

### Android
1. 安装 APK 后，在游戏库中选择存放本地游戏的文件夹（需授予文件访问权限）。
2. 点击游戏封面或卡片即可开始游玩；长按条目可切换「自动 / 日语原文 / 本地汉化」语种模式。

### Windows
- **直接游玩**：使用构建好的 `cs2-player.exe` 打开游戏目录：
  ```powershell
  cs2-player.exe window "游戏目录路径"
  ```
- **手动构建**：
  - Windows：`python -B scripts/build/cs2_workspace.py build --integration`
  - Android：`python -B scripts/build/android_workspace.py build --integration`

## 外挂汉化配置（`cs2-localization.json`）

如需在不覆盖原版资源的前提下加载汉化补丁，可在游戏根目录创建 UTF-8 编码的 `cs2-localization.json`：

```json
{
  "patches": [
    { "archive": "translation.int", "scene_encoding": "gbk" }
  ]
}
```

## 项目目录

| 目录 | 说明 |
| --- | --- |
| `simulator/crates/` | Rust 引擎核心（格式解析、脚本虚拟机、运行时、渲染器、桌面与 Android 桥接） |
| `simulator/android/` | Android 应用界面、游戏库与播放宿主 |
| `scripts/build/` | Windows 与 Android 构建脚本 |
| `scripts/convert/` | 游戏资源与汉化补丁转换脚本 |
| `scripts/analysis/` | PC 自动化回放测试、Android 性能分析与静态检查脚本 |

## 测试过的游戏

以下为目前已测试的 **20 款游戏**（当前主要通过 Windows 端批量回放与实机验证，其中 **19 款已完整跑通至少一条结局并完成系统存档冷读验证**，1 款推进测试中；Android 真机适配正在持续推进）：

1. ネコぱら vol.3（NEKOPARA Vol.3 Steam 版）
2. Happiness！2 樱花盛典
3. ISLAND（含 Android 真机序章与续读测试）
4. Yukikoi Melt
5. うたてめぐり
6. ここから夏のイノセンス！
7. カミカゼ☆エクスプローラー!
8. ツナバン らぶみくす
9. 尚有佳蜜伴恋心（恋ニ、甘味ヲソエテ）
10. 戦国天使ジブリール
11. 未来ラジオと人工鳩
12. 果つることなき未来より
13. 灰色的乐园 v1（グリザイアの楽園）
14. 灰色的果实（グリザイアの果実）
15. 灰色的迷宫（グリザイアの迷宮）
16. 牛顿与苹果树
17. 白昼夢の青写真
18. 祝福のカンパネラ（已测试章节剧情、战斗、鉴赏与自动存档，结局推进中）
19. 突然之间发现我已恋上你
20. 秘密游戏：反叛（汉化版 A 线）

> 另包含 CatSystem2 官方引擎示例 `cs2_full_v401`（用于启动、基础存读档与设置项回归测试）。
