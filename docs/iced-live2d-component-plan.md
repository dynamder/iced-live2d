# iced 组件化计划（`iced-live2d`）

**状态：设计已定，待批准后开工。** 这份文档是设计决策的完整记录，用来核对：术语、API 契约、
被否决的方案、分步验收。任何与本文冲突的实现都算偏离，先改文档再改代码。

- 前情：spike 阶段的可行性、性能与 mocari 缺陷见
  [`rust-iced-live2d-plan.md`](rust-iced-live2d-plan.md) 与
  [`mocari-upstream-report.md`](mocari-upstream-report.md)。
- mocari 继续 vendor：
  `.refs/mocari` = crates.io 0.4.1 + `patches/mocari-0.4.1-*.patch`，`scripts/vendor-mocari.ps1`
  重建。本轮不改 vendoring 方式。
- 目标：把"一个窗口里画一个 Live2D 模型"做成**可复用的 iced 组件**，用法与 iced 原生控件同构；
  现有 `app` 退化为示例宿主。

---

## 1. 术语：用 Cubism 的原话

| 概念 | 采用的名字 | 出处 / 理由 |
|---|---|---|
| 让角色看向某一点 | **LookAt** | 官方 SDK 教程 [Eye tracking settings](https://docs.live2d.com/en/cubism-sdk-tutorials/lookat/)："the Cubism SDK uses a component called **LookAt**"，组件族 `CubismLookController` / `CubismLookParameter` / `ICubismLookCenter` / `ICubismLookTarget`；其中的目标坐标归一化为 `-1..=1`，与我们的约定一致 |
| 从光标产生 LookAt 目标 | **`look_at::cursor`** | 官方把这件事建模成 `ICubismLookTarget::GetPosition()`，所以它是一个"现成的 target 实现"，不是一个"跟随行为"。不再叫 `follow_cursor` / `gaze` |
| 追踪参数写入口径（按 rig / 照抄 Viewer） | **`LookAtMode`**（原 `TrackingMode`） | 对应官方 `CubismLookController` 的 Blend Mode 概念：决定把 LookAt 写成谁。变体 `Adapted` / `ViewerDefault` 不变 |
| 鼠标拖拽（官方 web sample 的叫法） | 不采用 | 官方 sample 走的是 `onDrag` / `setDragging`，但那是"鼠标拖拽"这一具体输入，不是能力的名字；我们保留 `look_at` 作为能力名 |
| 加载期 GPU 资源预算 | **`RenderSettings`** | 不能叫 `ModelSettings`：Cubism 语境里 "model setting" 专指 `model3.json`（`CubismModelSettingJson`） |
| 模型对象 | **`Live2dModel`** | 不用 `Handle`：`use` 之后裸 `Handle` 看不出是什么 |
| 控件 | **`Live2d`** / 构造函数 `live2d(&model)` | 与 iced 的 `canvas(&…)/Canvas`、`text_editor(&…)/TextEditor` 同构 |
| 事件 | **`Live2dEvent`** | — |

### 消息命名：Subject-Verb-Object

依据 lustre 指南 [02 State management · "Messages not actions"](https://lustre.hexdocs.pm/guide/02-state-management.html)：
消息按 **主语 + 动词 + 宾语** 命名，主语是"谁发来的"，不用祈使句（那是 action 的风格）。

```gleam
// lustre 指南里的例子
UserUpdatedPassword(String)
UserRequestedPasswordReset
BackendResetPassword(Result(Nil, String))
```

落地到我们的事件（主语 = 模型 / 动作，发送方是组件）：

```rust
pub enum Live2dEvent {
    ModelLoaded { summary: ModelSummary },
    ModelFailedToLoad { failure: LoadFailure },
    ModelReportedProblem { problem: Problem },
    MotionFinishedPlaying { motion: MotionId },
}
```

映射到宿主后读作 `Message::Live2d(Live2dEvent::ModelFailedToLoad { .. })`。

**这条约束只管消息类型**。`Live2dModel` 上的方法保持祈使句（`play_motion` / `look_at` / `pause`）：
Rust 里方法名表达"用户意图"是本职，把方法写成 `user_requested…` 只会更难用。

---

## 2. 架构约束：来自 iced 的四个事实

设计不是偏好，是被下面四条钉死的（行号对应 iced `e5c350f`）：

| 事实 | 位置 | 后果 |
|---|---|---|
| `Primitive::Pipeline` **按 Primitive 类型共享一份**：`Storage` 是 `FxHashMap<TypeId, Box<dyn Pipeline>>`，`store::<P, _>(P::Pipeline::new(..))` | `iced_wgpu/src/primitive.rs:190,200` | 多模型必须由 pipeline 内部按 id 建 map，v1 就要有 |
| `Primitive::prepare` 返回 `()`，参数里**没有 `Shell`** | `iced_wgpu/src/primitive.rs:24-32` | GPU 侧发现的事件（加载失败、问题）无法直接发消息，只能经一条 `Sync` 通道回到下一帧 `update` |
| `Program::update` 是唯一能 `Action::{publish, request_redraw_at}` 的地方；`RedrawRequest::At` 由 winit 精确调度 | `widget/src/shader.rs:104-120`、`widget/src/action.rs`、`winit/src/window.rs:310` | 组件可以**自驱动帧**，宿主不必再订阅 `window::frames()`；iced 的闪烁光标就是这么写的（`core/src/text/editor.rs:355`） |
| `Pipeline::trim()` 每帧末尾必被调用 | `iced_wgpu/src/lib.rs:145` → `engine.rs:61` → `primitive.rs:218` | 有"帧末"钩子，用来回收已失去模型的 scene（RAII 落地点） |

iced **没有**任何自己的 GPU→UI 通道封装；它内部遇到同类需求时用的就是 channel
（`iced_wgpu/src/image/cache.rs:390-408` 的 `mpsc::sync_channel`）。它把整个 `futures` crate
重导出为 `iced::futures`，其中 `channel::mpsc::Sender` 是 `Sync`，但内部是 `Mutex` 且是 async 形状，
因此不采用。

**结论：事件通道用 `crossbeam-channel`**（`Sender: Sync + Clone`，无锁 MPMC，同步 drain），
Phase 3 的后台解码也复用它。

---

## 3. 线程模型：零锁

所有 iced 侧回调（`update` / `draw` / `prepare` / `render` / `trim`）都在 UI 线程按序执行，
所以"共享可变状态"只有两处：**模型的配置与命令**（app 线程写、组件读）和**事件**（GPU 侧写、
widget 侧读）。前者用单线程容器，后者用通道，全链路没有锁。

| 层 | 归属 | 可变性 | 进 `Primitive`？ |
|---|---|---|---|
| `Live2dModel` = `Rc<Inner>` | app 的 model | `Cell` / `RefCell`，`!Send`、`!Clone` | 否，只交出**纯数据快照** |
| `Live2dPrimitive` | iced 的 layer | 无内部可变状态 | — |
| `Live2dPipeline` | iced 的 `Storage`，单线程 `&mut` | `HashMap<u64, Slot>` | — |
| `State` | widget 树，单线程 `&mut` | crossbeam `Receiver` + 重绘记账 | — |

`Primitive` 携带的纯数据（全部 `Send + Sync`，不需要锁）：

```rust
pub struct Live2dPrimitive {
    id: u64,
    model_path: Arc<PathBuf>,
    settings: RenderSettings,
    look_at: Option<[f32; 2]>,      // 本帧的目标（来自 look_at 函数或 model.look_at）
    commands: Vec<Command>,          // 本帧要执行的动作
    physics_enabled: bool,
    paused: bool,
    presence: Arc<Presence>,         // RAII 令牌
    events: crossbeam_channel::Sender<Live2dEvent>,
}
```

时间：`prepare` 里用 `Instant::now()` 自测 delta。`Live2dScene::advance` 已经是固定 1/60 s 步进，
时钟不精确只会少走一步，不会破坏姿势，所以组件不需要宿主喂时间。

---

## 4. API 契约

### 4.1 构造：默认值只能出现在调用点

```rust
/// 一个模型的加载期资源预算。改它意味着重新加载。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSettings {
    /// 贴图上传倍率。0.5：4096² 级别 atlas 在几百像素高度上看不出来，显存省一半。
    pub texture_scale: f32,
    /// 遮罩 render target 边长。只有遮罩边缘出现异常时才需要调。
    pub mask_size: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self { texture_scale: 0.5, mask_size: 1024 }
    }
}

impl Live2dModel {
    /// 直接给一份完整设置。唯一真正干活的那个。
    pub fn from_path(model_path: impl Into<PathBuf>, settings: RenderSettings) -> Self;

    /// 链式风格；`.build()` 内部就是 `Self::from_path(self.model_path, self.state.0)`。
    pub fn builder(model_path: impl Into<PathBuf>) -> Live2dModelBuilder<NoSettings>;
}
```

`from_path` **不读文件**：解码与上传发生在第一次渲染。构造不 IO 沿用 iced `image::Handle::from_path`
的惯例；所有失败以 `ModelFailedToLoad` 事件报告。

### 4.2 builder：类型状态，缺设置不能编译

```rust
pub struct NoSettings;
pub struct WithSettings(RenderSettings);

pub struct Live2dModelBuilder<State> {
    model_path: PathBuf,
    state: State,                    // 状态参数直接携带数据：没有 Option、没有 PhantomData、没有 unwrap
}

impl Live2dModelBuilder<NoSettings> {
    /// 补上基础设置，进入 `WithSettings`。
    pub fn settings(self, settings: RenderSettings) -> Live2dModelBuilder<WithSettings>;
}

impl Live2dModelBuilder<WithSettings> {
    /// 覆盖单项，其余沿用 `settings()` 传入的那组。
    pub fn texture_scale(mut self, texture_scale: f32) -> Self;
    pub fn mask_size(mut self, mask_size: u32) -> Self;
    /// 冻结配置。
    pub fn build(self) -> Live2dModel;
}
```

调用点（默认值永远可见）：

```rust
Live2dModel::from_path(path, RenderSettings::default());

Live2dModel::builder(path).settings(RenderSettings::default()).build();

Live2dModel::builder(path)
    .settings(RenderSettings { texture_scale: 0.25, ..Default::default() })
    .build();
```

### 4.3 运行期控制（`&self` 链式，`!Clone`）

```rust
impl Live2dModel {
    /// 让角色看向控件坐标系里的一点：`-1.0..=1.0`，原点在控件中心，`+y` 朝上。
    /// 具体写哪几个参数由 rig 在加载时解析（见 `LookAtMode`）。
    pub fn look_at(&self, target: [f32; 2]) -> &Self;

    /// 追踪参数写入口径。切换会清掉旧的参数覆盖并重新解析 rig。
    pub fn look_at_mode(&self, mode: LookAtMode) -> &Self;

    pub fn set_physics_enabled(&self, enabled: bool) -> &Self;

    /// 暂停：组件不再请求重绘，也不再推进时间。
    pub fn pause(&self) -> &Self;
    pub fn resume(&self) -> &Self;

    pub fn play_motion(&self, motion: Motion) -> &Self;
    pub fn play_expression(&self, expression: Expression) -> &Self;
    pub fn stop_motion(&self) -> &Self;

    /// 还有多少命令没被消费（诊断用）。
    pub fn pending_commands(&self) -> usize;
}
```

### 4.4 控件

```rust
/// 构造控件。默认：`Fill` × `Fill`、不跟光标、`fill = 0.925`、不发消息。
pub fn live2d<'a, Message>(model: &'a Live2dModel) -> Live2d<'a, Message>;

impl<'a, Message> Live2d<'a, Message> {
    /// 事件回调。不给就不发消息（同 `Button::on_press` 的可选语义）。
    pub fn on_event(self, on_event: impl Fn(Live2dEvent) -> Message + 'a) -> Self;
    /// 每帧从光标取 LookAt 目标。显式传入，默认不跟。
    pub fn look_at(self, source: LookAtSource) -> Self;
    /// 取景：模型 canvas 在控件里占多大。1.0 = 内切，默认 0.925。
    pub fn fill(self, fill: f32) -> Self;
    pub fn width(self, width: impl Into<Length>) -> Self;
    pub fn height(self, height: impl Into<Length>) -> Self;
}
```

- `impl Program<Message> for Live2d<'_, Message>`：回调存 `Option<Rc<dyn Fn(Live2dEvent) -> Message + 'a>>`
  （`Box` 不行：`Widget` 转发需要一个可克隆的临时值）。
- `impl Widget<Message, Theme, Renderer> for Live2d<'_, Message> where Renderer: shader::primitive::Renderer`：
  **委托给 iced 的 `shader::Shader`**，不复制它的 tag/state/layout 逻辑。因此只支持 wgpu 后端
  （tiny-skia 后端无 `shader`）。
- 用法与原生控件同构：

```rust
live2d(&self.hiyori)
    .on_event(Message::Live2d)
    .look_at(look_at::cursor)
    .width(Fill)
    .height(Fill)
    .into()
```

### 4.5 LookAt 输入

```rust
/// 控件内光标位置 → LookAt 目标。光标在控件外返回 `None`。
pub type LookAtSource = fn(mouse::Cursor, Rectangle) -> Option<[f32; 2]>;

pub mod look_at {
    /// 现成的 target 实现：把光标映射成 `-1..=1` 的目标（`+y` 朝上）。
    /// 用法：`.look_at(look_at::cursor)`——`fn` 项本身就满足 `LookAtSource`，不需要常量包装。
    pub fn cursor(cursor: mouse::Cursor, bounds: Rectangle) -> Option<[f32; 2]>;
}
```

三条语义（写进 doc）：

1. 每帧在 `draw` 里被调用一次（只有那里同时有 `mouse::Cursor` 和 `bounds`；宿主拿不到 bounds）。
2. 返回 `Some(p)`：本帧 LookAt 目标就是 `p`；返回 `None`（光标在控件外）：回落到
   `Live2dModel::look_at` 设过的值，没设过则保持默认朝向。
3. 传自定义函数即完全接管（例如只跟随水平方向、加阻尼）。是 `fn` 指针而非闭包，没有所有权噪音。

### 4.6 事件与载荷

```rust
pub struct ModelSummary { pub motions: usize, pub motion_groups: usize, pub expressions: usize }
pub struct LoadFailure { pub path: PathBuf, pub error: Error }      // Error = live2d_render::Error
pub struct Problem { pub error: Error }
pub struct MotionId { pub index: usize }
```

- 载荷携带**类型化的错误**（`live2d_render::Error`），不转成 `String`：宿主可以 `match`。
  若该类型不满足 `Send + Sync`（通道要求），退化为 `Arc<Error>` 或字符串——实现时确认并在此更新。
- 加载状态不在模型上暴露：宿主用 `ModelLoaded` / `ModelFailedToLoad` 自己维护 UI 状态
  （Elm 的单一真相在宿主）。
- 事件**晚一帧**到达：`prepare` 无 `Shell`，只能在下一次 `update` 里 `publish`。
- 非致命问题（坏 sidecar、越界索引、命令队列溢出）走 `ModelReportedProblem`，同一条原因只报一次。

---

## 5. 决策记录（含否决项）

| # | 决定 | 被否决的方案与原因 |
|---|---|---|
| 1 | 事件名走 SVO | `Loaded` / `Failed` / `Problem` / `MotionFinished`：省略主语和宾语，最难读 |
| 2 | 方法名保持祈使句 | 把方法也写成 SVO（`user_requested_motion_playback`）：与 iced 原生控件风格更远，调用点变长 |
| 3 | `Live2dModel` | `Handle`：`use` 之后无信息；`Model`：与其它 domain 的 Model 易撞名 |
| 4 | `RenderSettings` | `ModelSettings`：Cubism 的 "model setting" 专指 `model3.json`；`SceneOptions`：那是 renderer 内部装载参数，不该整体暴露 |
| 5 | `look_at` / `look_at::cursor` | `gaze`：6 个槽里只有 2 个是眼睛，且 `set_gaze` 是渲染层内部叫法；`follow_cursor`：那是一个 target 实现，不是行为 |
| 6 | 光标跟随**默认关闭**，显式 `.look_at(look_at::CURSOR)` | 默认跟随：与 iced "显式优先"哲学冲突，且用户看不见这个行为 |
| 7 | 控件实现 `Widget`，`.into()` 走 iced 的泛型转换 | 自定义 `impl From<Live2d> for Element` + `.map()`：不是 iced 的写法 |
| 8 | 委托给 `shader::Shader` 实现 `Widget` | 复制 `Shader` 的 70 行实现：两份真相 |
| 9 | `from_path` + typestate builder **都保留** | 只留 builder：缺设置只能靠运行时 `Err`；只留 `from_path`：失去链式风格与逐项 override 的可读性（用户选择两者并存） |
| 10 | builder 的 base 必须显式传入（`builder(path).settings(..)`） | `builder(path, settings)` 或带默认值的 builder：后者会掩盖未设置的字段 |
| 11 | `WithSettings` 上不提供替换整组的 `settings()` | 用户要"部分默认"时，在 `RenderSettings` 构造处用 `..Default::default()` 表达，同一能力不出现两条路径 |
| 12 | 事件通道用 `crossbeam-channel` | `iced::futures::channel::mpsc`：内部是 `Mutex` + async 形状；`Mutex<Vec<_>>`：渲染路径上引入锁 |
| 13 | 多实例：一个模型一个 `Live2dModel`，pipeline 按 id 建 map | "一个对象装多个模型"（stage）：需要 per-model 变换、共享 mask target、按模型命中测试，是 `live2d-render` 的新能力 |
| 14 | 组件自驱动帧（`request_redraw_at(now + STEP)`） | 要求宿主订阅 `window::frames()`：每个用组件的 app 都要写一遍，且与 `RedrawRequest` 的"后设置者胜"耦合 |
| 15 | 时间由组件自测（`prepare` 里 `Instant::now()`） | 宿主传入 `Instant`：多一层 API，且 `advance` 的固定步进本来就容忍不精确的时钟 |
| 16 | `Live2dModel` 不实现 `Clone` | 可克隆：RAII 语义会模糊（"谁拥有它"），Elm 里它本来就在宿主 model 里 |
| 17 | 事件载荷用类型化错误 | `String`：丢掉可 `match` 的信息 |
| 18 | 控件方法与模型方法同名 `look_at` | 前者是"每帧从哪儿取目标"（`Live2d::look_at(LookAtSource)`），后者是"目标是多少"（`Live2dModel::look_at([f32;2])`）：同一个动词，参数类型区分，读起来是同一件事 |
| 19 | 不把 macOS/Linux 之外的平台拆开 | 组件只依赖 iced + wgpu，平台问题沿用 iced |

---

## 6. 分步实施

每一步都必须独立可验证、独立提交，验不过不进下一步。

### Step 1 · `live2d-render` 术语对齐与两个新入口（不改行为）

| 改动 | 位置 |
|---|---|
| `set_gaze` → `look_at`（doc 引官方 LookAt 教程） | `crates/live2d-render/src/scene.rs`；调用点 `app/src/live2d.rs`、`examples/{sweep,jolt,offscreen}.rs` |
| `TrackingMode` → `LookAtMode`，`SceneOptions::tracking` → `look_at_mode` | `rig.rs`、`lib.rs`、`app/src/cli.rs`、`examples/{rig,shared/knobs}.rs` |
| 新增 `Live2dScene::set_look_at_mode(LookAtMode)` | `Scene` 存 `ModelFacts` + 当前 mode；切换时 `clear_parameter_overrides()` 后重新 `rig::resolve` |
| 新增 `Live2dScene::set_fill(f32)` | 与 `resize` 共用同一条 `fit_matrix` 计算 |

验收：`cargo test` / `clippy` 干净；`offscreen` 8 个模型逐像素与改动前一致；`sweep`/`jolt` 输出数值不变；
`app --model <任一>` 行为不变。
提交：`refactor(render): call the look-at input by its Cubism name`

### Step 2 · 组件 crate 骨架（不碰 GPU，可单测）

```
crates/iced-live2d/
  Cargo.toml        iced(git, rev e5c350f) + live2d-render(path) + crossbeam-channel
  src/lib.rs        crate doc + live2d() + re-exports
  src/model.rs      Live2dModel / Live2dModelBuilder / RenderSettings / Command 队列 / Presence
  src/event.rs      Live2dEvent / ModelSummary / LoadFailure / Problem / MotionId
  src/look_at.rs    LookAtSource + look_at::CURSOR
```

要点：`Inner` 里 `look_at: Cell<Option<[f32; 2]>>`、`commands: RefCell<VecDeque<Command>>`
（上限 64，溢出丢最旧并计一次 `Problem`）、`presence: Arc<Presence>`；无锁。

验收（单元测试，无 GPU）：命令顺序与上限；`look_at` 最后写入值胜；builder 缺设置时
`build()` 不存在（编译期，用 `trybuild` 或直接注释说明，不引入新 dev-dep 时用文档说明）；
`from_path` 与 builder 产生等价模型（同一个 `Inner` 构造路径）。

提交：`feat(iced-live2d): model object, settings and look-at source`

### Step 3 · 控件 + primitive + pipeline

```
  src/widget.rs     Live2d<'a, Message>：impl Program<Message> + impl Widget（委托 shader::Shader）
  src/pipeline.rs   Live2dPipeline（HashMap<u64, Slot> + trim 回收） / Live2dPrimitive
  examples/minimal.rs
```

- `update`：drain 接收端 → `Action::publish`；`RedrawRequested(now)` → 未暂停时
  `Action::request_redraw_at(now + STEP)`。
- `draw`：从 `&Live2dModel` 取纯数据快照（含 LookAt 目标、命令、`LookAtSource` 的结果）放进 Primitive。
- `prepare`：按 id 找/建 `Slot`，`Instant::now()` 算 delta 调 `advance`；加载失败 → 推事件 + 状态置
  `Failed` 不再重试。
- `render`：只实现 `render`（不用 `Primitive::draw`）：遮挡需要自己的 mask pass，无法在 iced 的 pass 内嵌套。
- `trim`：`Weak<Presence>` 失效 → 释放该 scene 的 GPU 资源。

验收（8 条，逐条跑）：① `examples/minimal` 显示 Hiyori 且动作在播；② `.look_at(look_at::CURSOR)`
下鼠标移动头眼跟随、移出控件回落到 `model.look_at`；③ `R`/`E` 经命令队列生效；④ 同窗口两个模型各自
独立；⑤ 按键 drop 一个模型 → 下一帧 scene 释放、显存回落；⑥ 大肥鱼 ≤5 ms/帧、Haru ≤1 ms/帧
（`LIVE2D_PROFILE`）；⑦ 不存在路径 → `ModelFailedToLoad` 到达宿主并被打印；⑧ `clippy` 0 警告、
`fmt` 干净、所有 pub item 有 doc。

提交：`feat(iced-live2d): render a model as a shader widget`

### Step 4 · `app` 迁移 + 文档

`app` 缩到 <80 行，`--track` 映射到 `look_at_mode`，去掉 `window::frames()`（证明自驱动帧有效），
README 增加 "as a component" 一节并更新目录结构。

验收：8 个模型逐个开窗、`--list` 无窗口、大肥鱼 profile 不倒退、画面与迁移前一致。
提交：`refactor(app): host the model through iced-live2d`

### 不在本轮范围

异步加载（Phase 3，通道已就位）、命中测试（hit area）、部件透明度、`look_at_damping`
（官方 `CubismLookController` 有 Damping，我们暂缺，扩展点已留）、一个 `Live2dModel` 挂多个控件、
wasm。

---

## 7. 质量约束（每步都按这个检查）

- 库代码 **不许 `unwrap` / `expect` / `panic!`**；`#![deny(unsafe_code)]`、`#![warn(missing_docs)]`。
- 非法状态不可表达：`Slot` / 命令 / `Live2dEvent` 都是枚举；builder 用类型状态。
- 每个 pub item 有 doc，写"怎么用 + 坑"，不复述签名。
- 不用 `mod.rs`；模块以 `xxx.rs` + `xxx/` 组织。
- 每步有一个人类可跑、可区分的可执行文件。
- 错误必须暴露：任何失败都变成事件或 `Result`，绝不静默（本项目的 18 ms 教训）。
- 文档单源：API 的真相在 rustdoc，仓库层面的用法在 README，本文件只记决策与计划。

---

## 8. 已知代价与风险

| 项 | 说明 |
|---|---|
| 事件晚一帧 | iced 的 `prepare` 没有 `Shell`，这是结构性的，不是实现缺陷 |
| typestate 与条件构造 | `let b = builder(p); if flag { b = b.settings(..) }` 两个分支类型不同会编译失败；绕法是无论如何都传一组设置（默认值显式化，符合原则） |
| 只支持 wgpu 后端 | `Widget` 要求 `Renderer: shader::primitive::Renderer`；tiny-skia 后端没有 `shader` |
| `Live2dModel: !Send` | `Rc` 让它在 UI 线程用；要跨线程传的是路径，不是模型 |
| drop 后晚一帧释放显存 | scene 在下一帧 `trim` 回收 |
| 自驱动帧与宿主重绘请求共享槽位 | `RedrawRequest` 是单槽，"后设置者胜"：用本组件时不要再订阅 `window::frames()` |
| 多控件共享一个模型 | 同一帧会被 prepare 两次；`Stepper` 保证第二次是无操作。v1 允许但写入文档 |

---

## 9. 实现期待验证（不改变设计，但可能微调）

1. `live2d_render::Error` 是否 `Send + Sync`（决定事件载荷是 `Error` 还是 `Arc<Error>`/`String`）。
2. 自驱动帧与 iced 的重派发保护（已核对，留作实现时的对照）：`window::Event::RedrawRequested(Instant)`
   是 tuple variant，携带 `window.tick(frame_time)`（`iced_winit/src/lib.rs:772-774`）；`iced_winit`
   在 `lib.rs:784-815` 对"该事件产生了消息或 overlay 失效"会再派发该事件，最多 3 次后打
   `3 consecutive RedrawRequested events produced invalidation` 警告。`Shell::request_redraw_at`
   只写 `redraw_request`（`core/src/shell.rs:145-152`），不产生 invalidation，因此**自驱动帧本身不会**
   触发重派发；实现时要守的纪律是：**不要每帧 `publish` 消息**，只在真有事件时发。
3. `Shader` 的 `Widget` 方法转发在 `Rc` 回调下的可克隆性（`Live2d: Clone` 是否成立）。
4. `crossbeam-channel` 版本锁定（当前 0.5.x）。
