# CatSystem2 Simulator Workspace

本目录保存 CatSystem2 模拟器、只读游戏样本、格式研究、运行时证据和按日期归档的分析输出。当前文档状态见 [`reports/interface_ledger.md`](reports/interface_ledger.md)。

## 目录说明

- `simulator/`：Rust workspace；保留六个核心/Windows crate，新增 Android C ABI 宿主 crate。
- `simulator/crates/android/`：复用现有播放器的 Android `cdylib`、会话、像素和输入接口。
- `simulator/android/`：Android APK 工程；应用、Java 目录选择/SurfaceView、JNI 桥分别位于 `app/`、`app/src/main/java/org/catsystem2/player/`、`app/src/main/cpp/`。APK 不内置游戏。
- `simulator/crates/renderer/`：可移植逻辑画布、图层合成、遮罩、粒子和确定性帧快照。
- `simulator/vendor/ttf-parser-0.21.1/`：固定字形解析依赖，含 CFF2 轮廓闭合修复及原始许可证。
- `simulator/THIRD_PARTY.md`：外部参考的许可证与使用边界；KatzeSystem2 不作为运行时依赖。
- `scripts/analysis/`：样本盘点、PE 函数清单、Ghidra/Capstone 汇总和差分报告脚本。
- `scripts/analysis/run_playthrough_batch.py`：PC 全样本剧情验收；从正常入口回放输入，分游戏隔离测试存档并归档日志和画面。完成运行不代替可见对话和推进的人工确认。
- Windows 按钮复测先给 PC 批跑器传入 `--button-probe --scenarios scripts/analysis/pc_button_titles.json`，再让 `scripts/analysis/run_button_batch.py` 用同一回放器和 `--probes` 指定的归档目录逐个点按。用 `--scenarios scripts/analysis/pc_button_followup.json --indices 10` 探测《战国天使》的剧情右键菜单。每例归档独立测试档案、点击前后截图和鼠标回执，动作结果需核对截图及日志。
- 固定帧回放的 `tap:帧` 可把同帧鼠标点击改为同一逻辑帧内按下和释放。`pc_button_djibril_short_exit.json` 覆盖从其他位置移入退出按钮时的短点击与退出确认画面。
- `scripts/analysis/run_djibril_save_roundtrip.py --player <回放器>` 在两个独立进程里验证《战国天使》的普通槽写入、冷读和翻页，归档槽文件校验、输入回执及读回前后的正文图。
- `scripts/analysis/run_yukikoi_save_roundtrip.py --player <回放器>` 从 Yukikoi Melt 的右键菜单写普通槽，新进程从标题选 LOAD，只带入写出的槽文件，并记录确认和翻页。
- `scripts/analysis/run_button_batch.py --player <回放器> --cases <清单>` 运行命名按钮用例。可以重复传入 `--cases`，各清单共用构建、资源限额和结果目录。清单 `pc_button_title_actions.json`、`pc_button_settings_actions.json`、`pc_button_save_panel_actions.json`、`pc_button_legacy_scroll_cases.json` 分别覆盖已登记的标题与确认、设置、存档页和滚动控件。
- `scripts/analysis/review_button_captures.py <归档目录> --indices 1 2` 将已完成用例的前后截图并排输出到对应的运行目录，便于集中核对可见动作。
- `scripts/convert/`：离线转换工作区已有游戏资源。`restore_pc_story_10_scene.py` 校验并生成 10 的 `scene.int`。`build_hatsumira_font.py` 根据 Windows UIF 配置生成《果つることなき未来より》的配套字体。
- `simulator/crates/desktop/examples/story_replay.rs`：同一套引擎源码的 PC 回放入口。Windows 使用 `python scripts/build/cs2_workspace.py build --integration --story-replay` 构建，由正式流程接入 Common Controls 清单。
- `scripts/analysis/replay_emote_performance.py` 用 `--player` 复用归档回放器，直接读取 `--game-root` 原游戏，只把 `--seed-profile` 中的配置/存档复制到隔离 profile；`--workers 1|2|4`、`--emote-gpu 0|1` 和 `--row-pool 0|1` 用于同场景对照，路线用 `--drop-input`／`--append-input` 显式调整，回执保存完整参数、产物身份和截图哈希。
- 固定帧日志探针可同时使用 `capture-every:0` 和 `no-final-capture`；`inspect-log:` 记录合成层和等待状态，不保存画面。显式 `capture:` 或 `inspect:` 仍保留画面，按状态回放保留异常证据。使用 `story-events` 时可重复加 `trace-native:编号`，保留指定调用的原始参数。
- 状态场景可用 `diagnostics` 数组附加 `trace-native:编号` 和 `inspect-log:帧号`。它们只记录调用、等待以及影片解码和合成时间戳，不改变输入时机；状态路线仍不接受固定帧按键或点按。`composited_pts` 是画布合成证据，不代表平板实际呈现。
- 固定帧的 `require-hit:帧:区域` 必须对应同帧点击；实际命中的启用区域不符时停止并记为 `input_missed`，不会发出点击。
- 每个批跑回执区分首个引擎报错、场景配置错误、输入未命中、帧停滞和超时时仍在推进。若回放器输出了位置或守卫停机日志，回执同时记录最后观测到的场景页位置和停机详情。到达最后一个检查点记为 `checkpoint_reached`。结束卡、回标题和通关状态均经验证的路线记为 `ending_verified`。
- `scripts/analysis/plan_island_bad02.py --bad02-ending` 生成 ISLAND 从新游戏到 `BAD_02` 并回标题的路线，输出 JSON 可传给批跑器的 `--scenarios`。
- 分组复跑可重复传入 `--scenarios`，并用 `--indices` 选定游戏；这些游戏的场景必须各由一份清单提供，遗漏或重叠会在启动前报错，所有分组共用 `--jobs` 和内存、磁盘限额。
- 首轮通关批次启用 `--skip-cleared`。批跑器从相同缓存目录读取已归档的 `ending_verified` 回执，确认游戏、构建、结束卡和日志后写入 `skipped_cleared`，并将进程配额留给其余游戏。补分支或复验指定构建时省略该参数。
- 剧情快进批次使用 `--fast-text --reuse-attempts`。前者只调整隔离档的原生正文速度。后者在构建、游戏资源、路线、起始槽及文字设置一致时复用已完成检查点或已诊断停点，回执保留原日志入口。构建或路线更新会重新验证。复跑指定结果时省略 `--reuse-attempts`。
- 后台批次可用 `--admission-wait-seconds` 在内存或磁盘余量不足时等待资源恢复，再从队列启动下一款游戏。
- `scripts/analysis/run_neko_save_roundtrip.py --player <PC回放器> --to 485` 使用同版候选分别验证日文和繁中。各自从新游戏写入私有普通槽，再单独启动一次，通过游戏内菜单读同一槽并核对正文与输入日志。
- `reports/pc_playthrough_additional_games_20261120.json`：在旧启动清单之后补入《秘密游戏：反叛》（编号 20），默认批跑 01–20；V401 单独指定 `--indices 0`。
- `scripts/analysis/pc_story_checkpoints.json`：ISLAND 开场逐页回放与 V401 首个消息检查点，使用上面的批跑脚本。输入时序和证据见 [`reports/pc_story_checkpoints_20261118.md`](reports/pc_story_checkpoints_20261118.md)。
- `scripts/analysis/pc_story_batch_02_04_09_12.json`：四款游戏的 PC 正常入口回放。帧记录、下一检查点和当日 `archive/2026-11/` 回放及探针目录见 [`reports/pc_story_batch_02_04_09_12_20261118.md`](reports/pc_story_batch_02_04_09_12_20261118.md)。
- [`scripts/analysis/pc_story_sidecar3_01_02_04_05_06_07_20261119.json`](scripts/analysis/pc_story_sidecar3_01_02_04_05_06_07_20261119.json): hash-pinned PC route inputs for six samples. The [dated evidence index](archive/2026-11/20261119_pc_story_sidecar3_01_02_04_05_06_07.md) links the isolated 02/05/06 replay directories, receipts, and 02 Common Controls fixture under `archive/2026-11/`.
- [NEKOPARA sidecar 13 archive](archive/2026-11/20261119_neko_sidecar13_123612_2032/20261119_face13_findings.md): task 114 primitive order, stencil traces, and no-hand replay verification.
- [`reports/pc_story_08_10_14_20261119.md`](reports/pc_story_08_10_14_20261119.md)：08／10／14 的 PC 隔离回放、首个阻断和后续验收；日期归档保留输入、回执、截图及独立 profile。
- `scripts/analysis/host_source_syntax.py`：Android Java/JNI源码语法读取；不调用编译器、运行时或设备。
- `scripts/analysis/android_load_probe.py`、`android_surface_metrics.py`、`android_phase_metrics.py`、`android_flip_metrics.py` 记录画面加载、实际呈现帧间隔、播放器分段耗时和正文翻页响应。复测命令见 `reports/android_acceptance_20261118.md`。
- `scripts/analysis/android_story_replay.py` 对已确认的真机会话执行有限次点按，保存 APK 哈希、输入、截图和日志。用截图与原生输入日志核对实际剧情进度。
- `scripts/analysis/android_story_drive.py` 用只读 `CS2Story` 日志推进已确认的正文等待点，不逐页截图；真机执行方式见下文。
- `reports/android_story_round1_20261119.md` 记录真机剧情分支、存读档、批次源码与 APK 身份；PC 诊断不计入真机验收。
- `scripts/build/`：正式主流程；`cs2_workspace.py snapshot` 更新静态调用与工作量报告，`toolchain` 检查工具链；`build --integration` 用于当前整编，默认正式构建保留完整实现校验。
- `scripts/build/android_workspace.py`：平板/SDK 盘点及 APK 构建安装；显式 `--integration` 接入当前未完成实现的平板验证，单独记录源码、日志和产物身份。
- 同源验收先用 `cs2_workspace.py build --integration --story-replay --profile perf --freeze-source`，再用 `android_workspace.py build --integration --profile perf --replay-build <PC构建的build.json>`。APK 构建核对 PC 回放器、冻结源码和构建回执；设置 `CS2_BUILD_CACHE_ROOT` 时两端产物均按日期归档到该物理盘。性能对照使用 `perf`（opt-level 3、thin LTO、无增量），快速功能迭代用 `playthrough`；Android 也可用 `--profile release` 做对照。
- `scripts/build/sync_github_source.ps1`：将允许公开的引擎源码及相关工具同步到独立的 `CatSystem2-emulator` 签出，不自动提交或推送。
- Android 整编默认使用 `android-integration` Cargo profile，关闭增量、调试断言和溢出检查；正式发布仍使用 `release`。已有内容清单后可加 `--skip-media` 复用经计划与文件哈希验证的 FFmpeg 缓存。
- 已归档 APK 可用 `android_workspace.py install --integration --serial <设备> --exploratory --apk <APK路径>` 原位复现；该通道校验实际 APK 与同目录回执、记录源码和 profile，不要求匹配当前源码，也不标记为已验收。省略 `--exploratory` 的正式安装继续执行严格的源码与产物闸门。
- `resources/reference/`：固定版本的外部参考仓库、提交、许可证和使用边界。
- `reports/`：样本清单、格式矩阵、接口账本、函数账本、Ghidra 状态和 CLI JSON schema。
- `reports/workload_status_20261116.md`：按原版函数、调用槽位和完整游戏验收分别统计剩余量；不把静态覆盖率当作游戏完成率。
- `reports/remaining_to_full_games_20261118.md`：剩余两个交付包；Windows整编与已证雾兼容源码已完成，整图快照和未证动态目录后置，只优先实际游戏阻断、APK和平板/全样本验收。
- `scripts/analysis/format_B_d3d8_unwritten_fog_probe.c`：针对实际Campanella未写oFog阻断的最小系统D3D8对照；不使用原游戏程序或进入播放器产品。
- `reports/save_storage_20261118.md`：1031在创建/截断存档前查询实际私有目标卷空间；Windows与Unix/Android宿主源码、固定PE及分表调用证据。
- `scripts/analysis/implementation_gap_markers.py`：当前源码未恢复提示导航；静态snapshot自动归档源码/行号，提示或所在函数数量不等同剩余实现总数。
- `reports/island_image_patch_B_20261117.md`、`reports/model_fog_F_20261117.md`：ISLAND逐slice图片替换与固定旧版MDL顶点雾的实际源码消费者。
- `reports/fes_animation_wiring_C_20261117.md`、`reports/model_material_color_F_20261117.md`：正式FES动画更新与固定旧版模型颜色/查询的共享接线。
- `scripts/analysis/auxiliary_limit_contracts.py`：区分图片缩小枚举与设备尺寸上限；普通/转场捕获共享软件后端预算，证据直接归日期archive。
- `reports/batch_progress.md`：每批合入后的新增实现、调用缺口变化和未接通的主流程，持续向用户汇报。

### Android 正文回放

1. 使用包含 `CS2Story` 的验收 APK。运行 `adb -s $serial shell setprop log.tag.CS2Story D`，再打开或重开游戏。日志接口默认关闭，只报逻辑帧、场景标识、等待点与状态，不输出正文或文件路径，也不提供写存档、改变量或跳场景的接口。当前正文识别仅针对已识别的 ISLAND 样本；其他游戏不会自动点按。
2. 在平板上载入已确认的存档、手工选择分支并确认横屏触点。传入实际安装 APK 的 SHA-256 和 `CS2Player` 报告的精确游戏根路径。例如，ISLAND 选完「俺は俺だ」后，可用 `BAD_02 37 207`（零基组号和场景内绝对记录号）作为停点：

   ```powershell
   python -B scripts/analysis/android_story_drive.py --serial $serial --label island-bad02 --game-root /storage/emulated/0/CS2Games/ISLAND --apk-sha256 $apkHash --allow-scene com01_selC_1 --allow-scene BAD_02 --stop-at BAD_02 37 207 --max-actions 44 --timeout 240 --tap 160 1800
   ```

   此触点仅对应已确认的 `3200×2136` 横屏布局。脚本检查前台与会话、对每个确认等待点最多发一次触控，遇到选项、未知状态或越界停下。`timeout_live` 表示逻辑帧仍推进，可查等待、转场和触控回执；`timeout_no_heartbeat` 则先查会话、引擎日志和阻断。报告与停止截图放入当月 `archive/`；可用 `--capture-start` 额外保存起点截图。正式结局验收仍需核对结局画面及其后的标题回归。
3. 测试完成后运行 `adb -s $serial shell setprop log.tag.CS2Story S` 并重开游戏，关闭本会话采样。PC 回放可加 `story-trace` 检查同一套等待点判定，不计入真机通关。
- `reports/emote_debug_integration_20261117.md`：933实时M2登记值、CP932文本及Windows/Android共享剪贴板接线；保留底层变量查询及原版边缘差异的证据边界。
- `reports/fes_image_source_E_20261117.md`、`reports/model_draw_queue_F_20261117.md`：实际CG缩略图的资源/像素/输入回执，以及旧3D模型与外部粒子的共享排序和深度消费。
- `reports/fes_image_raw_B_20261117.md`：实际正文等待图标与用户字形的原始多帧来源、共享帧目录、释放及CPU5存档引用接线。
- `reports/settings_plugin_general_E_20261117.md`：General107四字段的私有配置保存和Windows/Android实际消费者；其余配置页按实际命中继续实现。
- `reports/settings_plugin_sound_E_20261117.md`、`reports/model_motion_light_F_20261117.md`：实际声音109页的焦点/声像消费，以及固定Campanella MDL动画的共享采样、冻结与绘制入口。
- `reports/unindexed_archives_B_20261117.md`：历史3个非KIF项的定点分类、ISLAND FUYINPAK图片包reader与现有资源索引接线；图片替换消费者另行收拢。
- `reports/standalone_data_save_20261117.md`：三整数、姓名、序列、原始HG、分组键值、记录栈和XML共用的完整状态捕获/准备/提交及allocator身份恢复；portable graph明确版本化。
- `reports/native_batch_snapshots.json`：主流程自动保留每次静态快照的精确新增编号、路由回退及源码证据状态。
- `reports/source_function_bindings_20261117.json`、`.md`：按原版SHA-256/RVA及普通表别名关联当前源码候选，保留跨版本未验证状态。
- `reports/rolling_snapshot_20261117.md`：并行修改时保留静态统计实际读取字节，明确当前源码匹配与归档可重现的区别。
- `reports/root_number_index_20261117.md`：77按实际根索引key查询，区别于工厂编号排除集合，并保留注销到根收集之间的状态。
- `reports/factory_null_profiles_20261117.json`、`.md`：固定PE工厂的原生空分支与对应KCS门控证据，禁止推广成未知版本的零值占位。
- `reports/batch_native_wiring_20261117.md`：按对象族的 ABI 描述表、共享接收器和批量接线流程及当前边界。
- `reports/batch_wiring_queue_20261117.json`、`.md`：剩余编号按固定PE包装及同编号源码接收者分批导航；由`scripts/analysis/batch_wiring_queue.py`随snapshot刷新，审查后才登记实际实现。
- `reports/native_file_objects_20261117.md`、`reports/native_string_search_20261117.md`：高频文件对象及 CP932 子串入口的固定 PE 证据与独立实现边界。
- `reports/android_host_20261117.md`：已连接平板、现有工具链、Android 宿主接线和全部实现后的 APK 验收顺序。
- `reports/android_auxiliary_ui_20261117.md`：真实消息列表界面、629异步平台回执及只读文档provider；均待整体实现后设备验收。
- `scripts/analysis/host_dialog_contracts.py`：445真实系统错误框的固定PE标题、import和包装证据，输出归reports。
- `reports/fes_settings_20261117.md`：FES getconf/setconf 与原始配置块、语言、声音及字体服务的实际接线。
- `reports/configuration_persistence_20261117.md`：独立配置写出、私有目录持久化、声音配置回读及全屏位置查询。
- `reports/save_scalar_batch_20261117.md`：原始数据/运动快照追加、容器克隆、log10及任务状态树格式的实际范围。
- `reports/graphics_canvas_integration_20261117.md`：持久逻辑画布与最终帧分离、渲染效果真实工作面及深度范围回调。
- `reports/sound_notice_integration_20261117.md`：SOUND真实通知、外层单语句effect及回执续行，保留alias生产链和NEXT/REV触发源恢复边界。
- `reports/frame_capture_20261117.md`：441连续帧捕获、565/566持久查询、真实最终画面和独立目录BMP写出；固定PE证据由`scripts/analysis/frame_capture_contracts.py`保存。
- `reports/emocon_native_20261117.md`：1055–1061任务独立标记配置、尺寸缩放、纹理及姿势偏移查询，固定PE与文档证据由scripts/analysis/emocon_native_contracts.py保存。
- `reports/shared_control_batch_20261117.md`：10/1039共用实际剪贴板提交、1077/1078与FES共享场景跳过字段、1079查询实际语言索引；scripts/analysis/shared_control_contracts.py固定相关PE证据。
- `reports/window_control_batch_20261117.md`：845–847实际窗口请求、任务关闭通知及Windows/Android消费；scripts/analysis/window_control_contracts.py固定包装与接收链，457/628及全屏附带行为仍待补全。
- `reports/native_context_return_20261117.md`：549原版未写返回槽与VM参数别名、12份PE的固定证据、独立32位VM身份和快照保留；scripts/analysis/native_context_contracts.py维护精确版本门控。
- `reports/residual_task_batch_20261117.md`：577根任务时钟保持、622/623原始图像注册及624真实ANM别名接收；scripts/analysis/residual_task_contracts.py记录固定PE、参数区和接收链证据。
- `reports/root_clock_profiles_20261117.json`：577按固定PE核对包装、根接收、更新后采样和计时器保持分支；`scripts/analysis/root_clock_profiles.py`生成12份精确版本的运行时命令描述。
- `reports/achievement_native_20261117.md`：1041–1044共用独立本地成就表与持久化，`scripts/analysis/achievement_native_contracts.py`固定四份PE接收链并生成描述；保留Steam账户同步缺口。
- `reports/sound_context_20261117.md`：声音类创建上下文的12份精确PE默认接收链，`scripts/analysis/sound_context_contracts.py`生成版本描述，取消有证据版本的额外拒绝。
- `reports/reserved_native_20261117.md`：40个保留入口的12份精确PE及480条完整常量函数字节绑定；`scripts/analysis/reserved_native_contracts.py`生成正式版本描述，snapshot核对实际分派结果。
- `reports/recovery_A_20261117_rolling_delivery.md`：type61 Effekseer的具体资源、manager、CPU纹理像素链及Emote小分支，保留未完整行为清单。
- `reports/emote_placement_20261117.md`、`reports/emote_placement_contracts_20261117.json`：NEKOPARA M2的84/93/101与51/55/66实际player位置、缩放、旋转和scaleoffset，按driver证据在根节点transformOrder之前消费；分析脚本为`scripts/analysis/emote_placement_contracts.py`。
- `simulator/crates/runtime/src/emote_driver_transform.rs`：M2播放器根属性覆盖与原f32角度归一化，供实际PSB采样器消费。
- `simulator/crates/runtime/src/emote_surface.rs`、`reports/emote_surface_20261117.md`：M2主差分/931及成功跨PSB替换的真实表面所有者、时钟冻结、透明RGB处理与像素淡出；固定PE证据由`scripts/analysis/emote_surface_contracts.py`保存，secondary与失败重开仍单列。
- `simulator/crates/runtime/src/emote_m2force.rs`、`emote_m2settings.rs`、`reports/emote_force_20261117.md`：M2实际位置命令／变量惯性、同组资源切换一次冲量、整数步弹簧、独立累加器、真实材质组插值／启停及driver变量消费；`desktop/src/emote_settings.rs`接FES到已载和后载角色，`scripts/analysis/emote_force_contracts.py`记录固定PE及14份配置结构。
- `reports/settings_menu_input_volume_E_20261117.md`：设置输入/音量子页、Reset共享状态和系统音量两平台消费者；完整插件页、角色播放表和硬件joypad仍在继续。
- `reports/settings_font_E_20261117.md`、`reports/settings_sound_roles_E_20261117.md`：两平台字体选择/像素预览、角色多选与逐字段声音Reset，保留原版具名查找缺陷及其它配置页缺口。
- `reports/css_effect_F_followup_20261117.md`：真实马赛克行展开及长时长级别实现；旧CSS材质接收证据与尚缺owner分开记录。
- `reports/android_tweet_B_20261117.md`、`reports/tweet_legacy_ui_B_20261117.md`、`reports/image_records_profiles_B_20261117.md`：Android Tweet消费者、原凭据只读导入/编辑计数与703精确PE分派，未执行网络操作。
- `simulator/crates/desktop/src/particle_plugin.rs`、`particle_rain_field.rs`、`particle_burst.rs`：固定SHA的独立PPI模型，共用真实随机流、时钟、纹理及Canvas消费；其余插件逐家族恢复，证据见`reports/desktop_recovery_D_20261116.md`。
- `reports/tweet_native_B_20261117.md`：type62六个入口的拥有消息、Windows授权/编辑/HTTPS工作线程、原版状态和生命周期，平台及兼容缺口单列。
- `reports/image_records_compat_B_20261117.md`：703默认PE兼容策略、V401分层/动画原版副作用及其他版本未证明边界。
- `reports/css_task_576_F_20261117.md`、`reports/frame_effect_resample_F_20261117.md`：576固定PE早退/空工厂与仍缺正文接收、两版真实效果缩放核，不据此计576完整实现。
- `reports/css_task_receiver_F_20261117.md`：576真实owner拒绝/输出保留和3D释放路径更正，旧DAE正文消费者仍在恢复。
- `simulator/crates/desktop/src/particle_family.rs`、`particle_family_profiles.rs`：PPI经精确SHA选择共享语义核心，覆盖dark/ground/question家族并接现有粒子生命周期与真实绘制。
- `simulator/crates/desktop/src/particle_family_path.rs`、`particle_family_sprite.rs`：共享轨迹/尾迹与单精灵阶段核心；`scripts/analysis/particle_ppi_manifest.py`维护131个资源身份与实际消费链，`snapshot`自动刷新到剩余工作量报告。
- `reports/calendar_rectangle_20261117.md`：786原始日历算术与787实际逻辑设备矩形，保留DWORD回绕及输出别名顺序。
- `reports/cst_context_save_recovery_B_20261117.md`：CST语言绑定、历史附加数据、数值/字符串快照与恢复的正式入口。
- `reports/startup_inventory_20261116.md`：20 个样本的配置入口和归档资源对应关系，只读核对。
- `reports/katze_api_reuse_20261116.md`：KatzeSystem2 公开 API、直接复用优先级、宿主接线及版本差异。
- `reports/runtime_integration_20261116.md`：独立运动/粒子 VM、数据服务、资源路由、文字和窗口的正式整合记录及证据边界。
- `archive/`：按 `YYYY-MM/` 归档的临时输出、构建缓存、逆向日志和阶段状态文档。
- `archive/2026-11/20261118_integration/`：首次 Windows 整编日志及退出记录；后续整编按日期保存，不能作为完整游戏通过证明。
- `archive/2026-11/20261118_12_native213_probe/`：样本 12 的 PC 表达式兼容补丁、隔离快照和对照测试；回放路径见批次报告。
- `archive/2026-11/20261119_08_native257/`：样本 08 的 PC 图像列表短参数 ABI、基于 05 的候选补丁及独立回归；见[交接记录](archive/2026-11/20261119_08_native257/20261119_08_native257_handoff.md)。
- `archive/build-cache/`：ASCII 工作区的默认本地构建缓存。Windows 中文路径工作区改用同卷的 `cs2-build-cache-<工作区标识>/`，供 GNU 链接器使用。历史 `archive/YYYY-MM/*_cache_ascii/` 是指向旧缓存的目录联接；停用联接和目标缓存可在构建全部结束后清理。
- `CS2_BUILD_CACHE_ROOT`：可选的工作区专用物理缓存根，须为绝对 ASCII 路径（例如 `F:\CS2-build-cache`）。默认及自定义缓存均按构建任务和阶段跨日期复用。成品与逐次日志仍按日期归档，游戏资源由原目录提供；PC 成品先写入按内容哈希命名的不可变 blob，再硬链接到日期归档，避免后续 Cargo 原地写入修改历史产物；Android 已结束的 staging 只保留最近两份且不清空活跃或身份不明目录。
- 并行修复期间可给 `cs2_workspace.py build --integration --story-replay` 加 `--freeze-source`。构建前将校验过的源码复制到物理缓存，构建回执记录副本路径和输入哈希。设置 `CS2_BUILD_CACHE_ROOT` 后，PC 回放器、构建回执和日志同存于该缓存根的 `archive/YYYY-MM/`。
- Windows 上连续构建时，复用同一个缓存根和用户级 Cargo 依赖缓存。`playthrough`、`android-integration` 和 `perf` 均关闭增量；构建脚本哈希单独记录但不参与运行时源码身份，单纯 touch 也不使产物失效，真实内容变化仍会触发重建。构建停止后，可清理 Cargo `target` 下各 profile 的 `build`、`deps`、`incremental` 和 `.fingerprint`，下次构建会重建。
- `archive/2026-11/source_blobs/`：静态接线统计实际读取的源码/输入字节，按哈希去重保存；用于并行修改期间重现候选绑定，不作为当前源码完成证明。
- `games/`：本机游戏和 `cs2_full_v401` 基准样本。每个样本直接放在 `games/<原目录名>/`，不再按引擎或系列分层；保留各游戏内部的资源、补丁和存档结构，不随 APK 或源码发布。
- `localization/`：本机汉化文件与变更记录。`campanella/` 存放《祝福的钟声》原版使用的 `update00.int` 和 `WenQuanYi_msgothic.ttf`，部署时两者放在原版游戏根目录，沿用原版 `cs2.exe` 和 `config/startup.xml`。
- `localization/果つることなき未来より/`：存放现有汉化的简体字形映射字体及 OFL 许可证。将 `HatsumiraSimplifiedChinese.ttf` 放入游戏根目录并重新打开该游戏，CatSystem2 Player 会从游戏目录读取字体。
- `localization/mirai-radio/`：存放《未来广播与人工鸽》的简中资源包和 `cs2-localization.json`。将两者放在汉化版游戏根目录，沿用 `cs2.exe`、原始资源与 `update01.int`。
- `localization/djibril5/`：存放《战国天使吉布莉尔》的松散汉化剧情、导出表、启动配置和配套字体。汉化目录同时需要可读的原版 `scene.int`。
- `localization/utatemeguri/`：存放《うたてめぐり》的 `update02.int`、替字表、由 `scripts/convert/build_hatsumira_font.py --profile utatemeguri` 生成的字体及 OFL 许可证。游戏从根目录读取补丁与字体。

模拟器直接读取用户持有的游戏目录和 `.int` 归档，不启动、注入、代理或分发原始游戏 EXE、DLL、游戏资源和官方工具。

### 补丁配置

在汉化版游戏根目录放置 UTF-8 的 `cs2-localization.json`，可以声明任意同引擎的补丁归档。补丁按列表顺序优先查找，未覆盖的内容继续走原有 `updateNN.int`、松散文件和基础归档查找顺序。可选的 `scene_encoding` 支持 `cp932`、`gbk` 和 `utf8`，只作用于从该补丁读出的 CST。

```json
{
  "patches": [
    { "archive": "translation.int", "scene_encoding": "gbk" }
  ]
}
```

日文原版沿用自己的目录和原始文件。汉化版使用自己的游戏目录及所需补丁、字体或松散文件。两个目录可作为游戏库的独立条目共存。对仅通过清单启用且名称不属于 `updateNN.int` 的补丁，也可以移走清单后重新打开同一目录，恢复原有资源顺序。

PC 剧情回放直接使用启动清单中的工作区原目录，同一版本只保留一份游戏资源；测试存档、配置、截图和日志写入独立的运行目录。诊断需要别名或改变资源解析时，记录映射或使用单文件替换，不复制整份游戏。Android 真机使用平板已导入的同版资源，更换 APK 不重复传游戏。

验收先按剧情推进到正式结局。用现有构建和回放日志复现阻塞，积累一段可验证的兼容性修复后再统一构建；不阻断剧情的一般优化后置。优先复用构建缓存，不为每个探针新建完整 `target`。

M2次级动作共用接线见 `reports/emote_secondary_20261117.md`；固定PE及资源证据由
`scripts/analysis/emote_secondary_contracts.py`维护，语法读取输出直接归日期archive目录。
同一流程接入911–916六个速度／权重入口，共用值表、真实driver计时及PSB消费者。
M2强制结束及活动淡变回收见 `reports/emote_finish_20261117.md`，普通更新与结束复用同一值表消费者。
M2配置样条的929/930与owned543/544共用资源、状态及力累加器，见
`simulator/crates/runtime/src/emote_m2spline.rs`、`reports/emote_spline_20261117.md`；
固定PE/资源证据由`scripts/analysis/emote_spline_contracts.py`维护。
M2属性查询910及装载滤镜918共用普通/owned消费者，见
`reports/emote_resource_controls_20261117.md`；固定PE、DLL及资源行由
`scripts/analysis/emote_resource_controls.py`维护，语法输出归日期archive。
M2全局932通过真实manager重算既有材质/运动/眨眼口型状态，见
`reports/emote_recompute_20261117.md`；证据脚本为`scripts/analysis/emote_recompute_contracts.py`。
M2动作1027/1047/1048及owned551/552/550共用真实action.def和持久步骤执行器，见
`simulator/crates/runtime/src/emote_m2action.rs`、`reports/emote_action_20261117.md`；
固定PE与28个实际资源动作由`scripts/analysis/emote_action_contracts.py`记录。
M2命中检测1025及owned548/549共用14份实际`#hittest`、原值表及力基准，见
`reports/emote_hittest_20261117.md`；证据脚本为`scripts/analysis/emote_hittest_contracts.py`。

同PSZ配置切换沿共享银行／chain及旧RGBA过渡，源码接入见
`reports/recovery_A_20261117_emote_profile.md`及`reports/recovery_A_20261117_emote_profile_timeline.md`；
timeline变化含transition0共用真实银行生命周期，不同base/layout仍保留明确边界。
926／owned534的实际secondary默认pattern查询见
`reports/recovery_A_20261117_emote_pattern_query.md`，普通VM与owned缓冲共用CP932输出链。
1029/1030真实touch描述符的CPU及双VM输出见`reports/emote_touch_integration_20261117.md`。
影片平面复用typed/owned消息与Body队列，旧701按精确PE接独占显示矩形，见
`reports/movie_plane_commands_20261117.md`；静态统计脚本归`scripts/analysis/movie_plane_command_contracts.py`。
旧Utate702按固定PE接BUTTON移动通知字段，普通／typed／owned／raw共用现有输入服务，见
`reports/legacy_plane_input_B_20261117.md`与`simulator/crates/desktop/src/legacy_plane_input.rs`。
真实物理声音的统一状态事务见`reports/physical_sound_checkpoint_E_20261117.md`；
已按最新授权进入首次整编与平板接入阶段，完整游戏实机验收仍待执行。
