# Android 正文翻页验收

进入已有稳定可读正文的场景后再采集。关闭自动和快进，每轮只点一次正文推进。无字序章留作剧情检查点，待正文可见后另起采集。以下命令从工作区根目录运行，设备相关命令由真机操作人执行。

## 离线自检与构建

```powershell
python -B -X utf8 scripts/analysis/android_flip_selftest.py -v
$flipClang = Join-Path $env:LOCALAPPDATA 'Android/Sdk/ndk/28.2.13676358/toolchains/llvm/prebuilt/windows-x86_64/bin/clang.exe'
$flipStamp = Get-Date
$flipRun = $flipStamp.ToString('yyyyMMdd_HHmmss_fff') + '_android_flip'
$flipArchive = Join-Path (Resolve-Path 'archive') $flipStamp.ToString('yyyy-MM')
New-Item -ItemType Directory -Force -Path $flipArchive | Out-Null
$flipExe = Join-Path $flipArchive ($flipRun + '_capture')
& $flipClang --target=aarch64-linux-android26 -std=c11 -O2 -Wall -Wextra -Werror scripts/analysis/android_flip_capture.c -o $flipExe
if ($LASTEXITCODE -ne 0) { throw 'Flip collector build failed' }
```

离线 Python 需要 Pillow。构建生成独立的 ARM64 shell 可执行文件，部署只涉及该文件。

## 设备采集

开启 `CS2Perf` 后从播放器打开游戏，推进到稳定可读正文并停住。每轮使用新的 `$flipRun`。下列第一段适用于首次开启记录的游戏会话。

```powershell
$flipSerial = '<tablet serial>'
$flipPerfBefore = (adb -s $flipSerial shell getprop log.tag.CS2Perf | Out-String).Trim()
adb -s $flipSerial shell setprop log.tag.CS2Perf D
```

第二段会给设备端进程最多 30 秒。终端出现 `READY` 后在平板正文区域点一次，等待 `SAVED`。设备逐张保存 PNG，采完后统一传输。起步用 24 张连续截图，按实测截图窗口和采集扰动调整样本数及间隔。进正文前后都用同一开机周期，保留同一个播放器进程直到导出日志。

```powershell
$flipRemoteExe = "/data/local/tmp/${flipRun}_capture"
$flipRemote = "/data/local/tmp/$flipRun"
$flipEvidence = Join-Path $flipArchive $flipRun
adb -s $flipSerial push $flipExe $flipRemoteExe
if ($LASTEXITCODE -ne 0) { throw 'Flip collector transfer failed' }
adb -s $flipSerial shell "chmod 700 $flipRemoteExe && timeout 30 $flipRemoteExe $flipRemote 24 0"
if ($LASTEXITCODE -ne 0) { throw 'Capture incomplete' }
adb -s $flipSerial pull $flipRemote $flipArchive
if ($LASTEXITCODE -ne 0) { throw 'Sample download failed' }
$flipPid = (adb -s $flipSerial shell pidof org.catsystem2.player | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $flipPid -notmatch '^\d+$') { throw 'Expected one live player PID' }
$flipLog = adb -s $flipSerial logcat --pid=$flipPid -d -v raw -s 'CS2Perf:I' '*:S'
if ($LASTEXITCODE -ne 0) { throw 'Player log export failed' }
$flipLog | Set-Content -LiteralPath (Join-Path $flipEvidence 'cs2perf.log') -Encoding UTF8
adb -s $flipSerial shell "setprop log.tag.CS2Perf '$flipPerfBefore'"
```

完成采集时，`capture.csv` 记录每张图的 `start_ns` 和 `end_ns`，分别取自设备 `CLOCK_MONOTONIC` 上调用 `fork` 前和 `screencap -p` 退出后。窗口包括调度、截图合成、PNG 编码和写入。中断时的 `capture.csv.partial` 仅供排查。

## 离线出报告

人工核对原始 PNG，选出旧正文和新正文完全可读的参考帧。在原始像素坐标里取包含不同文字笔画的固定区域，避开光标、动画和移动背景。本轮未捕获新正文时，可以把同一剧情、分辨率和布局下人工确认的新文本参考帧复制进证据目录。替换下面示例中的帧名、正文标识和区域坐标。`prepare` 从完整的 `capture.csv` 导入时间窗，并从同一进程的日志中定位窗口内唯一的 `primary_down`。

```powershell
python -B -X utf8 scripts/analysis/android_flip_metrics.py prepare $flipEvidence `
    --before-png frame_001.png --after-png frame_007.png `
    --before-text '旧正文摘录' --after-text '新正文摘录' --roi 400 1570 1360 1660 --reviewed
if ($LASTEXITCODE -ne 0) { throw 'Flip evidence preparation failed' }
$flipReport = python -B -X utf8 scripts/analysis/android_flip_metrics.py (Join-Path $flipEvidence 'flip.json')
$flipStatus = $LASTEXITCODE
if ($flipStatus -eq 2) { throw 'Flip evidence validation failed' }
$flipReport | Set-Content -LiteralPath (Join-Path $flipEvidence 'flip-report.json') -Encoding UTF8
$flipReport
```

- **点击证据**来自 `CS2Perf primary_down session=… input_mono_us=…`。原生日志向下截断到微秒，报告把它还原为 `[tap_start_ns, tap_end_ns] = [input_mono_us × 1000, input_mono_us × 1000 + 999]`。
- **首次新文字证据**是 `first_after` 指向的第一张与新正文参考区域完全一致的 PNG 和采集时间窗。延迟上界为其 `end_ns - tap_start_ns`，下界为 `max(0, last_before.start_ns - tap_end_ns)`。只对采样期间稳定的旧到新正文转换使用该区间。
- **判定**为 `PASS`（上界 ≤150 ms）、`FAIL`（下界 >150 ms）、`WINDOW_UNCERTAIN`（区间跨阈值或事件顺序不明确）或 `INSUFFICIENT_SAMPLES`（缺正文证据）。交付保留原始 PNG、CSV、日志、清单和报告。报告衡量原生收到按下到截图观察到新正文的时间窗。采集实效和扰动需要进入有字场景后测量，单轮报告对应一次点击。
