# 组件设计：`iced-live2d` 的收敛方案

> **English summary.** The settled design for the iced Live2D component: an
> immutable `Asset` (textures + parsed model + metadata) shared by independent
> `Model` instances; an `Interface` trait whose five `Vocabulary` enums are the
> compile-time vocabulary the host drives (with `Raw` as the degenerate
> "the model's own `ModelInfo` is the interface" escape hatch); raw-unit
> parameter/part tables applied at one point in the Cubism update order; a single
> non-generic GPU pipeline keyed by asset / model / view; model-level event
> broadcast; and load-time interface validation instead of per-call checks. The
> `Vocabulary` derive is optional and sits behind the `macros` feature.

**状态：设计定稿，作为实施依据。** 本文只回答「改成什么」；「为什么改」与逐项证据见
[`component-gap-audit.md`](component-gap-audit.md)。与之冲突的实现算偏离——先改本文。

- 目标：把它做成一个可复用的、类型安全的 iced 组件，而不是「一个能画模型的 demo」。
- 前提：尚未发布 0.1.0，**不做向后兼容**。旧类型直接删，`app` 与 examples 同步改。
- 约束：库代码不 `unwrap` / `expect` / `panic!`；每个 pub item 有 doc；不用 `mod.rs`。

---

## 1. 三条总原则

1. **非法状态不可表达。** 名字只在边界出现一次（加载时），内部全是类型化词汇；错误在加载时暴露，不在每帧。
2. **线程安全只花在必须的地方。** iced 的 `Primitive` 在 native 上要求 `Send + Sync`（`MaybeSend`/`MaybeSync` 的 native 实现就是 `Send`/`Sync`），因此只有每帧快照需要跨线程；模型、视图、覆盖表都是单线程普通结构，不用 `Arc`。
3. **单源。** 取景只有视图算，参数写入只有一个点，动作索引只有一个空间，贴图与元信息只有一个 owner。

---

## 2. 分层

```text
Live2dAsset               素材，加载后不可变
  · textures + moc3 + model3 + ModelInfo（canvas / 参数 / 部件 / 动作组 / 表情 / 命中区）
  · 动作路径表、两种 LookAtMode 的 rig 解析结果、模板 ModelRuntime
       │
       ├── Live2dModel<I: Interface>        实例，一个可独立驱动的状态机
       │     · ModelRuntime（参数值、部件、物理、pose）
       │     · 参数/部件覆盖表（键是 I 的词汇）
       │     · 播放器 / 表情 / Stepper / 命令队列 / idle 策略
       │
       └── Live2d<'a, Message, I>           视图，一个控件
             · 取景（fill / aspect → WgpuTransform）
             · 指针（命中、拖拽、悬停）
```

关系：

- 一个 `Live2dAsset` → 多个 `Live2dModel`：状态独立，共享贴图与解析结果。
- 一个 `Live2dModel` → 多个 `Live2d`：同步显示，各视图可各自取景。

渲染层的类型与上表一一对应：

```text
live2d_render::Asset    素材：textures + ModelInfo + 模板 ModelRuntime + 动作表
live2d_render::Model    实例：纯 CPU（runtime / 播放器 / 覆盖表 / stepper）
live2d_render::ModelGpu 实例的 GPU 镜像：mesh buffers / clipping / mask target
live2d_render::View     视图：WgpuTransform + 取景参数 + 坐标反变换
```

`Model` 是纯 CPU 的，因此 `step`、命中测试、参数读写都能脱离 GPU 单测；`ModelGpu` 是唯一需要
`device`/`queue` 的部分，由 pipeline 的 slot 持有。

加载预算拆成两个作用域：`AssetOptions { texture_scale }` 属于素材（贴图，共享），
`ModelOptions { mask_size }` 属于实例（每个实例一份 mask target）。旧的 `RenderSettings`
把两者混在一起，随本次重构删除。

---

## 3. 接口与词汇

```rust
pub trait Vocabulary: Clone + Eq + Ord + Hash + std::fmt::Debug + Send + Sync + 'static {
    const SPEC: VocabularySpec;

    fn name(&self) -> &str;
    /// Position in this vocabulary's own sequence.
    fn index(&self) -> usize;
    /// Rebuilds the key the render layer means. See [`KeyId`].
    fn from_id(id: &KeyId) -> Option<Self>;
    /// The asset a model-scoped key belongs to; `None` for compile-time keys.
    fn asset(&self) -> Option<AssetId> {
        None
    }
}

/// A vocabulary entry as the render layer identifies it.
pub struct KeyId {
    pub asset: AssetId,
    pub index: usize,
    pub name: Arc<str>,
}

pub enum VocabularySpec {
    Static(&'static [&'static str]),
    Model,
}

pub trait Interface: 'static {
    type Parameter: Vocabulary;
    type Part: Vocabulary;
    type Motion: Vocabulary;
    type Expression: Vocabulary;
    type HitArea: Vocabulary;
}
```

- `SPEC` 是关联常量而不是方法：`Interface` 零方法，pipeline 只需要一份纯数据
  `InterfaceSpec { parameter: VocabularySpec, .. }`，由泛型侧从 `I::*::SPEC` 拼出来放进
  primitive。**没有品牌参数，也没有「在 widget 里盖章」的环节。**
- `from_id` 是**反向**路径，不是冗余：事件里的键是渲染层发现的——`MotionFinishedPlaying
  { group }` 的组由播放器选出、`PointerEvent::Pressed { area }` 的命中区由 `hit_test` 选出，
  渲染层手里只有模型下标。`KeyId` 三个字段各有用途：`index` 是词表下标，`name` 让
  `Raw` 键能凭空造出来，`asset` 让外来键能被认出。缺了它，事件只能带字符串，类型化就是假的。
- `from_id` 对 derive 生成的实现恒为 `Some`；对手写实现可能 `None`（`SPEC` 的顺序与
  `index()`/`from_id()` 不一致）。那种情况发一条 `Problem`，不静默丢事件。
- `asset()` 只在 `VocabularySpec::Model` 下返回 `Some`，见 §3.2。
- `VocabularySpec::Static(names)`：加载时按名字解析成映射表，缺名字即失败。
  `VocabularySpec::Model`：映射是恒等（枚举序列就是模型自己的顺序），不可能缺。
- `index()` 的语义统一为「在该词表枚举序列中的位置」：typed 是 `ALL` 的位置，raw 是模型下标，
  两者在各自的 `SPEC` 下同序。
- `Vocabulary: Send + Sync` 是因为事件要经通道回到 UI 层；词表本身是纯数据。
- `Vocabulary: Debug` 是为了 `Live2dEvent<I>` 能 `Debug` 而不逼宿主给「接口标记类型」实现
  trait：Rust 的 derive 只会给类型参数加界，于是事件的 `Debug`/`Clone` 在组件里手写，界加在
  `I::Motion` 这些关联类型上；宿主那五个枚举 derive 一次就够，`struct Face;` 不必再 derive。
- 空词表是合法且有用的：`enum NoExpression {}` 表示「这个接口不提供表情」，derive 生成的
  `match *self {}` 让任何键都无法存在，正好是模型的能力。Hiyori 就是这种模型。

### 3.1 词表由宿主声明，宏是可选的糖

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Vocabulary)]
#[vocabulary(prefix = "Param")]
pub enum FaceParameter {
    MouthOpenY,
    EyeLOpen,
    #[name("ParamEyeBallX")]
    EyeBallX,
}

pub struct Face;

impl Interface for Face {
    type Parameter = FaceParameter;
    type Part = FacePart;
    type Motion = FaceMotion;
    type Expression = FaceExpression;
    type HitArea = FaceHitArea;
}
```

- derive 生成 `SPEC`、`name()`、`index()`、`from_id()`，以及一个**固有**的
  `const ALL: &'static [Self]`（宿主枚举用；不进 trait，免得手写实现被迫提供）。
  `prefix` 覆盖 Cubism 的标准前缀，逐变体 `#[name(..)]` 覆盖老 rig 的写法
  （`PARAM_ANGLE_X` 等），两者都不写就用变体名。
- `Interface` 只有五行 associated type，**手写，不另设 derive**；也不提供把五个词表藏进一条
  宏调用的 DSL。
- **宏糖在 `macros` feature 之后，默认关闭。** 关闭时组件不依赖 proc-macro crate，编译依赖零
  增加；需要 derive 的宿主（以及 examples）用 `features = ["macros"]` 打开。没有 feature 时，
  手写 `impl Vocabulary` 是等价路径，能力不打折。

### 3.2 键与可移植性

| 接口 | 参数键 | 可移植性 | 误用后果 |
|---|---|---|---|
| typed（如 `Face`） | 宿主声明的枚举变体 | 同一 `I` 的所有模型 | 编译错误 |
| `Raw` | `RawParameter { asset: AssetId, name: Arc<str>, index: usize }` | 仅本 asset | **插入时**按 asset id 拒绝，丢一次 `Problem` |

typed 的键就是枚举值，没有额外的 `ParameterKey`。`Raw` 是「任意模型 = 它自己给自己声明接口」
的退化实现：词表从加载出的 `ModelInfo` 枚举，宿主拿到的键带 asset 身份，换模型后旧键会被拒绝
并报一次问题，而不是静默写到同名下标。

过滤发生在 `wrap::Parameters::insert` 里（`Vocabulary::asset() != 本模型的 asset`），不是每帧
apply 时：覆盖表因此永远不可能持有外来键，也不必每帧扫一遍。

```rust
let parameters = raw::parameters(&info);
model.parameters_mut().insert(parameters[0].clone(), 0.5);
```

### 3.3 校验

- 只发生一次，在**加载时**，并且在**贴图上传之前**（校验只需要 moc3 + model3，失败时一兆显存
  都不花）。
- 失败走组件层的原因类型，不塞进 `live2d_render::Error`：

```rust
pub enum LoadFailure {
    Renderer { path: PathBuf, error: Arc<Error> },
    Interface { path: PathBuf, missing: MissingNames },
}

pub struct MissingNames {
    pub parameters: Vec<&'static str>,
    pub parts: Vec<&'static str>,
    pub motions: Vec<&'static str>,
    pub expressions: Vec<&'static str>,
    pub hit_areas: Vec<&'static str>,
}
```

- 严格：缺一个名字就失败。可变能力用更小的接口表达，不在一个接口里塞可选成员。
- 同一 asset 可以挂不同接口的模型，各自校验一次，贴图只加载一次。

---

## 4. 参数、部件、动作、表情

### 4.1 更新顺序

`Model::step` 是唯一写参数的地方，顺序对齐官方 `CubismUserModel::Update`：

```text
reset → motion → expression → 宿主参数 → look-at → 部件 → physics → pose
```

- **单位只有 raw。** 归一化是 UI 的事，`ModelInfo` 给了 min/max；clamp 由 mocari 负责。
- 宿主参数与部件表由实例自己持有并**每步重放**。因此 mocari 的
  `reset_parameters`（只恢复默认值、保留 override）与 `reset_part_opacities`（清空部件
  override）之间的语义差异，不再影响任何 API 承诺。
- look-at 折进同一步：rig 在加载期解析成「参数下标 + min/max」，每帧一次乘加。
  `write_normalized_override` 与 `clear_parameter_overrides` 全部删除——切换 `LookAtMode`
  不会再误伤宿主参数。

### 4.2 动作

- 索引空间回到官方形状 `(group, clip)`：`play_motion(group, clip)` /
  `play_random_motion(group)`；`MotionFinishedPlaying` 与 `MotionStarted` 也带这一对。
- 待机是显式策略，因为「默认」和「关闭」不能共用一个 `None`：

```rust
pub enum Idle<M> { Declared, Group(M), Off }
```

  `Declared`（默认）取模型声明的 `Idle` 组，组内随机（对齐官方）；`Group` 指定另一组；
  `Off` 即完全由宿主驱动。`Raw` 的 `Declared` 靠名字 `Idle` 解析，解析不到就是 `Off`。
- `duration` / `looping` 只有解析过动作文件才知道，因此随 `MotionStarted` 到达，不进加载期元数据。

### 4.3 表情

按接口枚举点播（`I::Expression`），不引入索引空间；表情的淡入淡出由 mocari 的
`ExpressionManager` 负责。

---

## 5. 帧、线程与 pipeline

```text
Live2dPipeline
├── renderer: WgpuLive2dRenderer          // 每渲染格式一份，跨所有 asset / model 共享
├── assets:  AssetId -> AssetSlot         // 贴图 + ModelInfo + 加载结果 + Presence
├── models:  ModelId -> ModelSlot         // Model + ModelGpu + 接口映射表 + Presence
└── views:   ViewId  -> ViewSlot          // WgpuTransform
```

- `Primitive` 只携带纯数据 + `Option<Sender>` + 两枚 `Arc<Presence>`（模型一枚、视图一枚，
  因为模型与视图各有自己的 slot 与回收条件）。**不设 `on_event` 时没有通道**，
  无界增长的问题从结构上消失。
- 事件走**模型级广播**：pipeline 按模型记 listener，首次注册时补发一次加载结果；`send` 失败即
  剔除（接收端随控件离开树消失），不需要帧计数。
- 每帧顺序：

| 阶段 | 职责 |
|---|---|
| `update` | drain 通道并发布事件；未暂停时排下一帧 |
| `draw` | 从 `&Live2dModel<I>` 生成 `FrameInputs`（覆盖表、look-at、暂停、物理、idle、命令、光标） |
| `prepare` | 加载/校验 asset；建 slot；reconcile 期望状态；step；上传；命中测试；发事件 |
| `render` | mask pass + model pass，使用该视图的 transform |
| `trim` | 回收失去 Presence 的 slot |

- 覆盖表每帧随 primitive 浅拷贝（几项，微秒级），**不用 `Arc`**：iced 的 layer 在 `update`
  期间仍持有上一帧 primitive，`Arc::make_mut` 照样会 clone，CoW 在这里没有收益。

---

## 6. 事件

```rust
pub enum Live2dEvent<I: Interface> {
    ModelLoaded { info: Arc<ModelInfo> },
    ModelFailedToLoad { failure: LoadFailure },
    ModelReportedProblem { problem: Problem },
    MotionStarted { group: I::Motion, clip: usize, duration: f32, looping: bool },
    MotionFinishedPlaying { group: I::Motion, clip: usize },
    Pointer(PointerEvent<I>),
}
```

- `ModelInfo` 是**非泛型**的 asset 视图（canvas、全部名字与范围、命中区列表），用于 UI 展示与
  `Raw` 枚举，**不签发键**。`ModelInfo::bounds` 同时是取景输入。
- 带键的事件在控件侧用 `I::*::from_id` 还原：渲染层发的是 `KeyId`，控件换成类型化键。还原失败
  只可能是手写 `Vocabulary` 与 `SPEC` 不一致，报一条 `Problem`。
- 事件晚一帧：`prepare` 拿不到 `Shell`，只能在下一次 `update` 发布。这是结构性的，不是缺陷。
- 多模型时宿主用闭包捕获下标区分（`live2d(&model).on_event(move |e| Message::Live2d(index, e))`）。

---

## 7. 视图与交互

### 7.1 取景

- 取景是**视图属性**：`View { transform, aspect, fill, bounds }`，`View::to_model` 提供命中
  测试用的反变换（由 `fit_matrix` 产出的仿射矩阵解析求逆，不另写一份取景数学）。
  取景逻辑单源，pipeline 不再保存 aspect/fill。
- `ModelInfo::bounds` 是模型空间包围盒，宿主用它按模型比例摆控件；`View` 也从这里拿取景输入。
- 多视图共享一个实例时各视图取景独立，**已核实成立**：mask 的 `mask_transform` /
  `clip_params` 只由模型空间几何与 mask target 布局决定（mocari 的 `ClippingContext`），
  与视图变换无关；且每个 primitive 的 mask pass 紧跟自己的 model pass。代价是同一模型 N 个
  视图时 mask pass 跑 N 次——正确，只是重复，暂不优化。

### 7.2 指针

语义化事件经同一个通道：

```text
PointerEvent<I>::
    Pressed  { area: Option<I::HitArea>, at: [f32; 2] }
    Released { area: Option<I::HitArea>, at: [f32; 2] }
    Entered  { area: I::HitArea }
    Exited   { area: I::HitArea }
    Dragged  { delta: [f32; 2] }
    Scrolled { lines: f32 }
```

- 命中在 `prepare` 里做完（`step` 之后，用当帧网格），事件晚一帧；悬停用 `draw` 拿到的光标，
  不需要请求/应答。`mouse_interaction` 用上一帧的命中结果。
- 拖拽只发 `Dragged`：拖窗口还是驱动物理由宿主决定；滚轮只发 `Scrolled`，缩放等取景变换那一层。
- `pause` = 全冻结（不推进时间、不请求帧），因此**暂停期间没有指针事件**。这是该语义的直接后果；
  要「停动作但仍可交互」用 `stop_motion()`。

---

## 8. 与审计缺陷的对应

| 审计项 | 本设计的答案 |
|---|---|
| `on_event` 不设时事件通道无界增长 | `Primitive.events: Option<Sender>`，没有监听者就没有通道 |
| 动作索引越界（计数跨组、点播单组） | 索引空间统一为 `(group, clip)`，组来自接口 |
| 一模型挂多控件事件只到第一个 | 模型级 listener 广播 + 首注册补发加载结果 |
| `look_at` 文档与实现矛盾 | 文档写死「视图提供的源优先，宿主值只在光标离开时生效」；标量策略留在模型 |
| `pause` 冻结 look-at | 保持「pause = 全部」，写进文档；交互式停止用 `stop_motion` |
| 自动接续与宿主点播差一帧 | 待机显式化（`idle_group`），需要精确调度时设 `None` 由宿主驱动；新增 `MotionStarted` |
| 参数/部件无写入路径 | 覆盖表 + 单点写入，raw 单位 |
| `set_look_at_mode` 清空宿主 override | look-at 改为每步重算，不再占用 override 表 |

---

## 9. 未定与明确推迟

| 项 | 状态 |
|---|---|
| 命中范围：只认 `model3.json` 的 `HitAreas`，还是加 drawable 轮廓 | 未定；先按前者 + 未命中回传坐标 |
| look-at 阻尼（官方 `CubismLook` 有 Damping） | 未定；`LookAtSource` 的文档矛盾无论如何都要修 |
| 多视图取景 | **已排除**：mask 变换与视图无关（§7.1），不需要 mocari 补丁；像素验证随步骤 5 做 |
| 动作优先级 / 队列 / crossfade | 推迟（mocari 现在还丢弃动作级 fade） |
| 呼吸 / 眨眼 / 口型同步 | 推迟（mocari 没有，需要自己写） |
| 异步加载 / 进度 / 重试 | 推迟 |

---

## 10. 落地

八步全部落地；下面记的是每步的验收证据，不是计划。

| 步骤 | 状态 | 证据 |
|---|---|---|
| 1. `live2d-render` 拆 `Asset`/`Model`/`ModelGpu`/`View`，删 `Live2dScene` | ✅ | 5 张 `examples/offscreen` 帧（Hiyori 静止 / look-at / viewer 模式 / 动作、大肥鱼静止）重构前后 **SHA-256 逐字节相同** |
| 2. 组件层三张表 / 事件广播 / `Option<Sender>` | ✅ | `examples/minimal` 两个模型各自收到 `ModelLoaded` |
| 3. `Interface` + `Raw` + 加载校验（校验先于贴图上传） | ✅ | `examples/typed` 在 Hiyori 上通过；在 Wanko 上报 `interface mismatch: missing parameters […]; missing parts […]`，一条而非每帧一条 |
| 4. 参数 / 部件 / 动作 / 表情 API 与 `step` 重排 | ✅ | look-at 帧与重构前像素相同（那条路径正是重排后的写入点） |
| 5. 取景移到视图；mask 变换与视图无关 | ✅ | mask 的 `mask_transform`/`clip_params` 只由模型空间几何与 mask target 布局决定；多视图因此不需要 mocari 补丁 |
| 6. 交互层（指针、命中、光标形状） | ✅ | 滚轮/点击经 `Live2dEvent::Pointer` 到达宿主（`examples/typed` 打印） |
| 7. `macros` feature 与 `#[derive(Vocabulary)]` | ✅ | 独立 crate `crates/iced-live2d-macros`；默认关闭 |
| 8. §9 推迟的子系统 | ⏸ | 未开始（见 §9） |

像素回归的命令与基线：

```bash
LIVE2D_TEXTURE_SCALE=0.5 LIVE2D_NO_MOTION=1 ./target/debug/examples/offscreen.exe <model> out.png 900 30
```

`LIVE2D_PROFILE=1` 现在把一帧拆成 `step` / `deform` / `upload` 三项；Hiyori（134 drawable）实测
`step 0.08 ms, deform 3.4 ms, upload 1.4 ms`，60 fps。
