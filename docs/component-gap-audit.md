# 组件能力审计：`iced-live2d` 现在缺什么

> **English summary.** An inventory of what the iced Live2D component actually
> exposes today and what is missing, measured against the vendored `mocari 0.4.1`,
> the official Cubism framework in `.refs/cubism-framework`, and the iced `shader`
> widget's hooks. The component exposes one interaction (cursor → LookAt), no
> model-data access (parameters, parts, hit areas, physics options), a single
> motion player (no groups, priority, queue or cross-fade), and a fixed 60 Hz
> redraw loop with a blocking synchronous load. The audit also records six
> behaviour defects, two of which are plain bugs: an unbounded event channel when
> `on_event` is not set, and a motion index that is validated against every group
> while it is resolved inside one group.

**状态：只记录现状、缺口与已核实的证据，不含方案定稿。** 讨论结论与实施步骤另起文档，
本文用来对照「实现是否真的补上了某条能力」。

- 审计对象：`ce8bdae` 工作树（`crates/iced-live2d`、`crates/live2d-render`、`app`）
- 对照物：`.refs/mocari`（0.4.1 + 本仓库补丁）、`.refs/cubism-framework`（官方 C++ 框架）、
  `.refs/iced`（`shader` widget 与 `Program` trait）
- 方法：逐项读公开面 → 找底层是否已有对应能力 → 用语料实测确认缺口在真实模型上成立
- 一条判据：**「底层有、渲染层没接、组件没暴露」算接线；「谁都没有」才算造轮子**

---

## 0. 一览

| 层 | 现在 | 缺 |
|---|---|---|
| 输入交互 | 1 条：光标 → LookAt | 点击/命中区、悬停、拖拽、滚轮缩放、双击、触摸、光标形状、事件捕获、焦点 |
| 模型数据 | 0 条（只有诊断入口） | 参数读写、部件透明度、命中测试、物理 gravity/wind/reset、动作/表情/组/参数的名字、canvas 尺寸 |
| 动作播放 | 3 条命令（按索引播动作/表情、停动作） | 动作组、优先级、队列、抢占、crossfade、loop 覆盖、seek、速度、当前动作读回、`MotionStarted` |
| 表情 | 按索引播 | 名字、清除、当前表情、权重 |
| 渲染取景 | `fill` 一个旋钮 | 整体 opacity、zoom/rotate/anchor/mirror、圆角裁剪、MSAA、DPR 感知 |
| 生命期 | 同步加载、失败永久 | 异步加载、进度、重试、热替换、idle 节流、`time_scale` |
| 多实例 | 多模型各自独立 | 一个模型挂多个控件时事件与命令不对称 |

**一句话：模型自己能动的（idle 动作、物理、pose）是自动跑的；宿主能施加的影响只有
`look_at` 和「按索引点播」。**

---

## 1. 公开面清单

### 1.1 `Live2dModel`（`crates/iced-live2d/src/model.rs`）

| 类别 | 方法 |
|---|---|
| 构造 | `from_path`、`builder(path).settings(..).build()`、`path`、`settings` |
| LookAt | `look_at([f32; 2])`、`look_at_mode`、`set_look_at_mode` |
| 时间 | `pause`、`resume`、`is_paused` |
| 物理 | `physics_enabled`、`set_physics_enabled` |
| 播放 | `play_motion(usize)`、`play_expression(usize)`、`stop_motion` |

### 1.2 `Live2d` 控件（`crates/iced-live2d/src/widget.rs`）

`on_event`、`look_at(source)`、`fill`、`width`、`height`。仅此五项。

### 1.3 其余公开类型

| 类型 | 内容 |
|---|---|
| `Live2dEvent` | `ModelLoaded{summary}`、`ModelFailedToLoad{failure}`、`ModelReportedProblem{problem}`、`MotionFinishedPlaying{motion}` |
| `ModelSummary` | `motions`、`motion_groups`、`expressions`（三个计数） |
| `Problem` | `Renderer{error}`、`CommandQueueOverflow{dropped}` |
| `Command`（crate 私有） | `PlayMotion(usize)`、`PlayExpression(usize)`、`StopMotion` |
| `RenderSettings` | `texture_scale`、`mask_size` |
| `LookAtSource` | `fn(mouse::Cursor, Rectangle) -> Option<[f32; 2]>`，加一个 `look_at::cursor` |

三个直接结论：

1. 交互面只有 LookAt 一项，**没有任何鼠标/触摸事件进得来**。
2. 命令面只有「按索引播」和「停」，**没有选择、没有参数、没有状态**。
3. 元数据只有计数，**所有「叫什么」的信息都停在渲染层**。

---

## 2. 缺失：交互层

| 交互 | 现在 | 缺的零件 | 底层 | 成本 |
|---|---|---|---|---|
| 点击 / 命中区 | 无 | 命中请求/应答 + `HitAreaPressed` + 坐标反变换 | mocari `ModelRuntime::hit_test`；`model3.json` 的 `HitAreas` 已解析 | M |
| 悬停 | 无 | 悬停状态 + 重绘策略 | iced 钩子齐全 | S |
| 拖拽 | 无 | 按下/移动/抬起状态机、增量、惯性、越界保持 | iced 钩子齐全 | M |
| 滚轮缩放 / 双击复位 | 无 | `WheelScrolled` 处理 + 变换状态 | iced 有事件；变换能力见 §5 | M |
| 触摸 | 无 | `Event::Touch` 分支 | iced 有 | S/M |
| 光标形状 | 永远箭头 | 覆写 `Program::mouse_interaction` | `Shader` 会转发给 program | S |
| 键盘 / 焦点 | 无 | `Widget::operate` + focus | iced 有 | M |
| 事件捕获 | 不捕获 | `Action::and_capture()` | iced 有 | S |
| 右键 / 长按 | 无 | 同点击 | iced 有 | S |

### 2.1 为什么说这层「不是架构限制」

`shader::Program`（`.refs/iced/widget/src/shader/program.rs`）已经提供：

- `update(&self, state, event: &Event, bounds, cursor)` —— 收到**全部** iced 事件，
  包括 `mouse::Event::{CursorMoved, ButtonPressed, ButtonReleased, WheelScrolled}` 与 `Event::Touch`；
- `mouse_interaction(state, bounds, cursor)` —— 默认 `Idle`，可覆写；
- `Action` —— `publish` / `capture` / `request_redraw(_at)`。

组件只用了其中「`RedrawRequested` 时申请下一帧」这一条，其余全部没写：
`widget.rs` 的 `update` 只匹配窗口重绘事件，`mouse_interaction` 原样转发给 `Shader`
（因此永远是 `Idle`）。

### 2.2 命中区有两件真麻烦

1. **坐标**：`hit_test(x, y)` 吃模型空间坐标，事件里是控件逻辑坐标，中间隔着取景变换
   （`ModelBounds::fit_matrix` 依赖 aspect 与 `fill`）。`Live2dScene::bounds` 是私有字段、
   没有 getter，所以「控件坐标 → 模型坐标 → hit_test」必须落在 `live2d-render` 里，
   否则取景逻辑会有第二份实现。
2. **时序**：`Program::update` 在 UI 线程、拿不到 pipeline；scene 只存在于
   `Primitive::prepare`。命中查询只能像 `Command` 一样排队、在 `prepare` 里
   （`advance` 之后、用当帧网格）执行、再经事件通道晚一帧回到宿主——和现有
   `Live2dEvent` 的语义同构。

### 2.3 事件载荷也缺东西

- 事件没有模型身份：`ModelLoaded` / `Problem` / `MotionFinishedPlaying` 都不带 path 或 id，
  多模型宿主只能靠闭包捕获下标（`minimal` 就是这么绕的）。
- 没有请求关联 id，将来做命中的请求/应答时无法把答复对上请求。
- 没有 `MotionStarted` / 当前动作 / 进度，UI 无法显示「正在播什么」。

---

## 3. 缺失：动作与表情的播放语义

| 缺口 | 现状 | 位置 |
|---|---|---|
| 动作组不可达 | `play_motion(i)` 只在 `Idle` 组（无 Idle 则字母序第一组）取下标，`TapBody` 等组永远播不到 | `scene.rs` 的 `preferred_motions` |
| 索引与计数不一致 | `motion_count()` 跨组求和，`play_motion` 只认一组 | `scene.rs` 的 `motion_count` / `play_motion` |
| 无优先级/队列/抢占 | 只有一个 `Option<MotionPlayer>`，后一次点播直接顶掉前一次 | `scene.rs` 的 `simulate` |
| 无 crossfade | mocari 的 `MotionPlayer::apply` 把动作级 fade 硬写成 `0.0`，文件里声明的 `FadeInTime`/`FadeOutTime` 被丢弃 | `.refs/mocari/src/motion.rs` |
| 无 loop 覆盖 / seek / 速度 | 只能跟随文件的 `Loop` 字段 | `MotionPlayer::new/new_once/with_looping` |
| 自动接续与宿主点播竞态 | 单次动作结束的同一 step 内就自动播下一个，事件晚一帧才到宿主 | `scene.rs` 的 `simulate` |
| 表情只能按索引 | 无名字、无清除（渲染层有 `clear_expressions`，组件没暴露）、无当前表情、无权重 | `model.rs` 的命令面 |
| 无 additive / multiply 混合 | 只有 override（替换）；官方 `AddParameterValue` / `MultiplyParameterValue` 在 mocari 也没有 | 官方 `CubismModel.hpp` |

---

## 4. 缺失：模型数据访问

| 能力 | mocari | live2d-render | 组件 | 后果 |
|---|---|---|---|---|
| 参数读 | `parameter_infos()`（id/min/max/default/value） | 仅诊断 `parameter_value` / `parameter_ids` | 无 | 做不了滑杆与调试面板 |
| 参数写 | `set_parameter` / `set_parameter_normalized` / `*_override` | 仅诊断 `set_parameter_offset` | 无 | 做不了口型同步、自定义表情、宿主驱动的姿态 |
| 部件透明度 | `set_part_opacity` / `part_ids` | 无 | 无 | 做不了「摘眼镜/换手/隐藏部件」 |
| 命中区 | `hit_test` / `hit_test_all` | 无 | 无 | 做不了点击 |
| 物理参数 | `physics_options` / `set_physics_options(gravity, wind)` / `reset_physics` / `stabilize_physics` | 只有总开关 | 无 | 做不了甩动、吹风、重力演示 |
| pose | 有，每帧强制执行 | 无开关 | 无 | 不能 A/B |
| 动作名 / 表情名 | `MotionReference` / `ExpressionReference::name()` | 只有计数 | 无 | 宿主只能显示 `#3` |
| 参数/部件的可读名 | `cdi3.json` 解析器存在，但没有任何代码加载它 | 无 | 无 | 做不了有意义的 UI |
| canvas 尺寸 / 包围盒 | 有 | `ModelBounds` 私有、无 getter | 无 | 宿主无法按模型比例摆控件、无法做锚点 |

---

## 5. 缺失：渲染与取景

| 缺口 | 现状 |
|---|---|
| 整体不透明度 / 淡入淡出 | 无；drawable opacity 由顶点自带，没有全局乘子 |
| 缩放 / 平移 / 旋转 / 镜像 / 锚点 | 只有 `fill`（内切比例）；`set_fill` 拒绝非正值，因此也没有镜像 |
| 非矩形裁剪 | 做不到：primitive 只拿到轴对齐 `clip_bounds`，外层 `container` 的圆角不会裁到模型 |
| 背景 / 描边 / 投影 | 无 |
| MSAA / mipmap 策略 / 色彩空间 | 都不是可选项；色彩空间是硬约束（`Bgra8Unorm`） |
| DPR 感知的贴图倍率 | `texture_scale` 是常数，不随窗口 DPI 走 |
| 后端 | 只有 wgpu；tiny-skia 不支持（已声明） |
| wasm | 不支持（已声明） |

---

## 6. 缺失：生命期、性能与多实例

| 缺口 | 现状 | 后果 |
|---|---|---|
| 同步加载 | 第一次 `prepare` 里直接 `load` | 首帧阻塞 UI 几百毫秒 |
| 无进度 / 无 loading 状态 | 只有 Loaded / Failed 二态 | 宿主只能自己猜 |
| 失败不可重试 | `Slot::Failed` 永久 | 一次 IO 抖动 = 永久黑屏，只能换对象 |
| 不能换模型 / 换 settings | `RenderSettings` 是 `Copy` 且不可变 | 切模型要重建 |
| 恒定 60 Hz 空转 | 未暂停时每个重绘事件都申请下一帧；`advance` 的「没东西可动」只省了模拟，没省绘制 | 待机也满帧跑 GPU |
| `pause` 冻结 look-at | 暂停时不请求重绘，且 `prepare` 在 `advance` 前返回 | 暂停后鼠标划过毫无反应 |
| 无 `time_scale` | 固定 1/60 步进 | 做不了慢动作 |
| 一个模型挂多个控件 | 第二个控件收不到 `ModelLoaded`；两者每帧互相覆盖 `look_at`；命令被先画的控件抽干 | 计划文档说「允许」，实际半坏 |
| 事件无模型身份 | 见 §2.3 | 多模型宿主只能绕 |

---

## 7. 底层盘点：接线还是造轮子

| 能力 | mocari | 渲染层已接 | 组件已暴露 | 判定 |
|---|---|---|---|---|
| 命中测试 | 有 | 否 | 否 | 接线 + 坐标反变换 |
| 部件透明度 | 有 | 否 | 否 | 接线 |
| 参数读写 | 有 | 仅诊断 | 否 | 接线 |
| 物理 gravity/wind/reset | 有 | 否 | 否 | 接线 |
| 动作组 / 表情名 / 组名 | 有 | 仅计数 | 否 | 接线 |
| DisplayInfo 可读名 | 解析器有、未加载 | 否 | 否 | 小接线 |
| pose 开关 | 有 | 强制开 | 否 | 小接线 |
| 命中区坐标反变换 | — | 缺 bounds getter + 反投影 | — | 需要设计 |
| 点击/悬停/拖拽/滚轮/触摸/光标 | — | — | — | 造轮子（钩子齐全） |
| 整体不透明度 | 顶点自带 opacity | 无全局乘子 | — | 小改渲染 |
| 缩放/旋转/锚点/镜像 | — | `fit_matrix` 可扩展 | — | 小改变换 |
| 动作 crossfade / 优先级 / 队列 | fade 助手有、player 丢弃 | 单 player | — | 造轮子 |
| 呼吸 / 眨眼 | 无（官方 `CubismBreath` / `CubismEyeBlink`） | 无 | 无 | 造轮子（各 30–50 行） |
| 口型同步 | 无 | 无 | 无 | 造轮子（要音频输入） |
| 异步加载 | 加载是同步 API | 同步 | 同步 | 造轮子（通道已就位） |
| 逐像素点击穿透 | — | — | — | 宿主级，不进组件 |

---

## 8. 已核实的缺陷

这些不是「特性缺失」，而是会被误当成缺失的行为问题。前两个是 bug。

### 8.1 不设 `on_event` 时事件通道无界增长

`next_message` 在 `on_event == None` 时用 `?` 直接返回，**发生在 drain 之前**；
而 `prepare` 无条件把 `ModelLoaded`、问题、完成事件写进这个
`crossbeam_channel::unbounded()`。不关心事件的宿主会缓慢泄漏。

### 8.2 动作索引越界

`motion_count()` 把所有组求和，`play_motion` 只在一组里取下表。Hiyori 是
`Idle: 9 + TapBody: 1`，`ModelSummary::motions` 报 10，但播第 10 个直接得到
`Error::MotionIndex { index: 9, count: 9 }`。两个宿主的 `R` 键都按
`% summary.motions` 循环，必然踩到。

### 8.3 一个模型挂两个控件时事件只到第一个

加载只在 `Entry::Vacant` 时发生，第二个控件的通道永远收不到 `ModelLoaded`；
`drain_commands` / `take_problems` 被先画的控件抽干；`look_at` 每帧互相覆盖。

### 8.4 `look_at` 的文档与实现矛盾

`look_at.rs` 说「想要阻尼/自定义规则，把状态放宿主侧，再用 `Live2dModel::look_at` 写」；
但 `widget.rs` 的解析是「source 有值就用 source，宿主值只作兜底」——**光标在控件内时
宿主写入被忽略**，而这正是最需要阻尼的场景。`LookAtSource` 是 `fn` 指针，也放不进状态。

### 8.5 `pause` 冻结 look-at

暂停 = 不再申请帧，且 `prepare` 在 `advance` 前返回。于是「停动作、仍看鼠标」做不到。

### 8.6 自动接续与宿主点播差一帧

单次动作结束的同一 step 内就 `play_next_motion()`，宿主要到下一帧才收到
`MotionFinishedPlaying`，于是「播完再接指定动作」会先闪一帧自动选的动作。

---

## 9. 语料实测

8 个官方样例 + 默认 rig 的普查（`model3.json` / `motion3.json` 直接统计）：

| 模型 | 动作组 | HitAreas | 表情 | 备注 |
|---|---|---|---|---|
| Haru | Idle 2 + TapBody 4 | Head, Body | 8 | |
| Hiyori | Idle 9 + TapBody 1 | Body | 0 | 计数 10 / 可播 9 |
| Mao | Idle 2 + TapBody 6 | Head, Body | 8 | |
| Mark | Idle 6 | — | 0 | 唯一没有 TapBody 与命中区 |
| Natori | Idle 3 + TapBody 5 | Head, Body | 11 | |
| Ren | Idle 1 + TapBody 2 | Body, Head | 5 | |
| Rice | Idle 1 + TapBody 3 | Body | 0 | |
| Wanko | Idle 3 + TapBody 2 | Body | 0 | |
| 大肥鱼（默认 rig） | 未声明 → 扫描 8 个文件 | 未声明 | 扫描 22 个 | 组名是合成的 `Scanned` |

- 8 个样例全部声明 `EyeBlink` 参数组，**没有任何代码驱动眨眼**。
- 50 个动作 **100% `Loop: true`**，所以 `MotionFinishedPlaying` 在这套语料上永不触发。
- 50 个动作里 **10 个声明了 0.25–1.0 s 的 `FadeInTime`/`FadeOutTime`**，全被 mocari 的
  `apply` 丢弃（动作切换是硬切）。

---

## 10. 复现

```bash
# 动作组与可播数量（CPU，不开窗）
./target/debug/examples/dump.exe .refs/cubism-web-samples/Samples/Resources/Hiyori/Hiyori.model3.json
./target/debug/examples/dump.exe .refs/cubism-web-samples/Samples/Resources/Wanko/Wanko.model3.json
./target/debug/examples/dump.exe "DeepSeek-fish-live2d/DeepSeek/大肥鱼.model3.json"
```

`dump` 末尾的 `motion groups:` / `expressions:` 段就是渲染层看到的组结构：
Hiyori 打出 `Idle: 9`、`TapBody: 1`；大肥鱼什么都不打（未声明，靠目录扫描）。

---

## 11. 与既有文档的关系

- `docs/iced-live2d-component-plan.md` 的「不在本轮范围」已经列了：异步加载、命中测试、
  部件透明度、`look_at` 阻尼、一个模型挂多个控件、wasm。本文确认这些仍是缺口，
  并补上它们之外的部分（播放语义、数据访问、渲染取景、性能）。
- `docs/rust-iced-live2d-plan.md` 已记「mocari 没有 breath / eye blink / lip sync，得自己写」，
  本文把它从「计划备注」变成「组件缺口」。
- 本文只记现状与证据；修哪些、按什么顺序修，由后续的讨论文档决定。
