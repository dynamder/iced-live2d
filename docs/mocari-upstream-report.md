# mocari 0.4.1 缺陷报告：四个已定位并验证的问题

> **English summary.** Four defects in `mocari 0.4.1`, each located to the exact
> lines, patched, and verified on real models. (1) A negative mask sentinel in a
> `.moc3` aborts model loading. (2) Textures are always uploaded without a mip
> chain, so minified line art breaks into speckles. (3) Physics outputs are never
> written back into the parameter cache, so chained physics settings do not
> propagate past the first hop. (4) Geometry is uploaded one drawable at a time,
> and a drawable whose mask set changes is treated as a fatal update error —
> together these cost a 715-drawable rig 18 ms of CPU per frame and halve its
> frame rate. Fixes in `patches/`, measurements below.

**状态：四个问题都已定位到具体代码行，都打了补丁并在真实模型上验证过。** 这份文档是给上游开
issue 用的素材：现象、根因（带代码与行号）、上游对照实现、修法、实测数据、复现方式。

- 版本：`mocari 0.4.1`（crates.io 原样，`.refs/mocari` = registry 副本 + 本仓库的补丁）
- 平台：Windows 11 · NVIDIA RTX 5060 Laptop · `wgpu 30.0.2` · iced `0.15.0-dev`（`e5c350f`）
- 宿主：iced 的 `shader` widget（自定义 pipeline），渲染目标 `Bgra8Unorm`，离屏验证用 `Rgba8Unorm`
- 模型：
  - `DeepSeek 大肥鱼`：VTuber rig，715 drawables / 58 967 vertices / 509 parameters /
    5 张贴图（未压缩 592 MB）/ 181 个动作 / 131 组 physics setting / 113 处跨 setting 读取。
    **非公开资产**，见第 6 节。
  - Live2D 官方样例 8 个（Haru / Hiyori / Mao / Mark / Natori / Ren / Rice / Wanko，
    Free Material License）——问题 2 可以直接用它们复现；问题 4 的"逐 drawable 上传"部分
    也能（任何 drawable 数上百的模型都够），动态遮罩那部分不行（见第 6 节）。
- 补丁（`scripts/vendor-mocari.ps1 -Force` 从 registry 重建后按文件名顺序应用）：

| 补丁 | 改动文件 |
|---|---|
| [`mocari-0.4.1-batched-vertex-upload.patch`](../patches/mocari-0.4.1-batched-vertex-upload.patch) | `render/wgpu/buffers.rs`、`render/wgpu/draw.rs` |
| [`mocari-0.4.1-chained-physics.patch`](../patches/mocari-0.4.1-chained-physics.patch) | `core/physics.rs` |
| [`mocari-0.4.1-mipmaps.patch`](../patches/mocari-0.4.1-mipmaps.patch) | `render/wgpu/pipeline.rs` |
| [`mocari-0.4.1-negative-masks.patch`](../patches/mocari-0.4.1-negative-masks.patch) | `render/common/clipping.rs` |

## 0. 一览

| # | 问题 | 影响 | 补丁规模 |
|---|---|---|---|
| 1 | 负遮罩哨兵值被当成索引 | 整个模型加载失败 | 3 行 |
| 2 | 贴图永远没有 mip 链 | 缩小后线稿碎成点状 | 生成链 + 两处参数 |
| 3 | physics 输出不写回参数缓存 | 物理链只走第一跳，次级部件静止 | 2 行 |
| 4 | 逐 drawable 上传 + 动态遮罩被当作致命错误 | 重 rig 18 ms/帧、35 fps | `buffers.rs` 重构 |

---

## 1. 负遮罩哨兵值导致模型加载失败

### 现象

本 rig 的 715 个 drawable 里有 **1 个** 的 mask 列表是 `masks == [-1]`（导出器用它表示
"无遮罩"）。mocari 把它直接送进 `usize::try_from`：

```
failed to create a clipping layout: invalid mask drawable index -1
```

整个模型打不开，没有局部失败这一说。

### 根因

`src/render/common/clipping.rs:25`（`DrawableInfo::from_mesh`）原样保留 `mesh.masks()`，
之后 `src/render/wgpu/pipeline.rs:56` / `:433` 把每个元素转成 `usize`：

```rust
usize::try_from(drawable_index)
    .map_err(|_| WgpuClippingLayoutError::InvalidMaskDrawableIndex { drawable_index })?;
```

### 上游对照

Cubism 的运行时对"没有被任何遮罩裁剪"的 drawable 就是不给遮罩，不写哨兵值；但导出器确实会写，
而官方 Viewer 打开这个模型正常，并报 **181 个被裁剪的 drawable**，而我们原始条目是 **182 个**
（1 个是 `-1`）—— 说明官方就是把负值当"无遮罩"处理。

### 方案

在 `DrawableInfo::from_mesh` 里过滤负值（`patches/mocari-0.4.1-negative-masks.patch`）：

```rust
let masks: Vec<i32> = mesh.masks().iter().copied().filter(|mask| *mask >= 0).collect();
```

### 实测

模型正常加载；被裁剪 drawable 计数与官方 Viewer 一致（181）；遮罩渲染与 Viewer 对照无差异
（用 `examples/probe` 逐点核对覆盖栈）。

---

## 2. 贴图永远没有 mip 链

### 现象

这个 rig 的贴图是 4096×8192、4096×8192、2048×2048、**8192×8192**、4096×4096，而桌面上只画
几百像素高，缩小倍数 4–8 倍。只有双线性采样时，线稿（1–2 px 的描边）会**碎成点状**，
细发丝和睫毛尤其明显。

### 根因

`src/render/wgpu/pipeline.rs:530` 建贴图时固定 `mip_level_count: 1`，采样器
（`:171`–`:180`）虽然有 `mipmap_filter: Nearest`，但没有任何 mip 级别可采。

### 上游对照

Cubism 的官方 Viewer/示例默认开启 mipmap（`CubismRenderer` 的 texture 参数与
`CubismModelSettingJson` 里都有开关），桌面尺寸下这是必须的。

### 方案

`patches/mocari-0.4.1-mipmaps.patch`：上传时在 CPU 上生成完整 mip 链（在预乘空间做 box
平均再反预乘，避免透明边缘出现暗边），采样器改成 `mipmap_filter: Linear`。

### 实测

线稿恢复连续。代价：贴图显存 +1/3（完整 mip 链就是 4/3；本 rig 半分辨率上传 148 MB，即约
+49 MB），启动多约 0.5 s（CPU 生成链）。

---

## 3. physics 输出不写回参数缓存

### 现象

`A → 中间参数 → 次级部件` 的链路里，只要中间参数是物理输出，**次级部件就完全不动**。
`examples/sweep` 以 60 fps 把追踪输入 `AngleX` 从 −1 扫到 +1：

```
frame     x   AngleX  ParamAngleX  ParamBodyAngleX  HipAngleX  HipZ1  FrontHairX1
    0 -1.000  -30.000     -28.002           0.000     0.000 -0.000       0.000
  239  1.000   30.000      27.750           0.000     0.000 -0.000       0.000
```

`ParamAngleX` 已经到 ±27.75，但 `PhysicsSetting50`（`ParamAngleX` → `FrontHairX1..X4`）的
输出恒为 0。反过来，**直接写** `ParamAngleX`（`SWEEP_DIRECT=ParamAngleX`）时 `FrontHairX1`
会出现 −0.737 —— 说明外部写进参数的值进得了物理输入，物理自己算出来的值进不了。

本 rig 有 **113 处**读取由别的 setting 产出的参数，所以这条链断了之后，整条物理链只走第一跳：
头发、尾巴、丝带、胸部、裙摆全都不动。

### 根因

`src/core/physics.rs:691`–`:703`（`evaluate_setting`）：

```rust
setting.current_outputs[index] = value;              // 算出来的值只留在这里

let Some((value, (&minimum, &maximum))) = parameter_cache.get_mut(parameter_index).zip(…) else {
    continue;
};
update_output_parameter(value, minimum, maximum, *value, output);
//                                              ^^^^^^ 这里应该是上一步算出来的 value
```

`value` 被 `get_mut` 的解构**重新绑定**成缓存槽本身，于是写回变成 `cache = blend(cache,
cache)` —— 恒等操作。

### 上游对照

`CubismNativeFramework/src/Physics/CubismPhysics.cpp:1003`：

```cpp
UpdateOutputParameterValue(
        &_parameterCaches[currentOutputs[i].DestinationParameterIndex],
        parameterMinimumValues[…] , parameterMaximumValues[…],
        outputValue,                    // ← 算出来的值
        &currentOutputs[i]);
```

`_parameterCaches` 就是"同一 substep 内后面 setting 能看到的输入"，框架在注释里写明了这点：
*"_parameterCaches needs to be separated from _parameterInputCaches because of its role in
propagating values between groups."*

### 方案

`patches/mocari-0.4.1-chained-physics.patch`：把 `*value` 改成解构前算好的 `target`/`value`
（两行）。细节与推导见 [`mocari-chained-physics.md`](mocari-chained-physics.md)。

### 实测

同一次 sweep：`ParamAngleX +27.750`，`FrontHairX1` 从 **0.000 → −0.136**；
`SWEEP_DIRECT=ParamAngleX` 时输出也正确（−0.737 → 归零行为符合框架）。
像素层面：转向时头发/裙摆/尾巴开始跟随，与官方 Viewer 的运动方向一致。

---

## 4. 逐 drawable 上传 + 动态遮罩被当成致命错误

这是四个里唯一一个"不是打不开、不是画错，而是**慢一半**"的问题，也是排查成本最高的：
它同时命中两个独立设计问题，而我们自己的错误处理又把线索吞掉了。

### 现象

715 个 drawable 的 rig，每帧 `advance` 的 CPU 成本 26–28 ms，`LIVE2D_PROFILE` 输出：

```
live2d profile: 60 frames 1.6 steps, rebuild 3.9 ms, upload 18.0 ms (mesh 17.2 over 715 drawables, clip 0.8)
```

帧时间 28 ms → 约 35 fps；因为场景按 1/60 s 固定步进，呈现跟不上时每帧要跑 1.6–1.9 步，
肉眼就是"轻微不连贯"。

### 根因

**（a）每个 drawable 一个顶点 buffer。** `src/render/wgpu/buffers.rs:360`
（`create_wgpu_drawable_buffers`）给每个 drawable 单独建 `vertex_buffer`/`index_buffer`，
`:245` 每帧对每个变了的 drawable 各调一次 `queue.write_buffer`。整模型变形时就是 715 次
staging 拷贝 + 715 条 copy 命令，2.6 MB 顶点要花 12–16 ms。

**（b）遮罩变化被当成"GPU 状态吸收不了"。** `update_drawables` 的校验
（`:303`–`:330`）在 `texture_index`、`blend_mode`、`masks`、`inverted_mask` 变化时返回
`WgpuMeshUpdateError`。而**动态遮罩就是会变**：本 rig 的 **drawable 373 每帧都换遮罩**
（20 秒里命中 1043 次）。调用方唯一的合法反应是整套重建 —— 于是每帧都重建 715 个顶点
buffer + 715 个 index buffer，这才是 18 ms 的主要来源。

这四个值根本不在顶点/index buffer 里：它们只影响绘制循环（贴图绑定、混合模式、用哪个
pipeline）和 clipping plan。所以"无法吸收"这个判断对它们并不成立。

**（c）我们这边也有一份责任。** 调用方把 `Err(_)` 吞掉了，只默默重建，于是"慢"没有任何线索。
把原因打出来后立刻看到 `drawable 373 masks changed`，问题定位只用了几分钟。

### 方案

`patches/mocari-0.4.1-batched-vertex-upload.patch`：

1. **共享顶点 buffer**：所有 drawable 按字节区间紧凑排布在一个 buffer 里，`from_drawables`
   通过 `mapped_at_creation` 一次性填充（该函数没有 queue）。一帧只写一个连续脏区间：
   `write_vertex_range` 把区间内各 drawable 的缓存字节拼起来，一次 `write_buffer`。
   `WgpuDrawableBuffers::vertex_buffer()` 由返回 `&wgpu::Buffer` 改成返回 `BufferSlice`
   （`drawable.vertex_buffer()` 直接交给 `set_vertex_buffer`，`draw.rs` 三处调用点相应简化）。
2. **这四个值改成就地更新**，并通过新增的 `WgpuMeshUpdate::structure_changed()` 上报；
   调用方只需重建 clipping 资源，不碰顶点/index buffer。
3. 复用一份 staging `Vec<u8>`，避免每帧分配/释放 2.6 MB。

### 实测（同一台机器、同一份模型、鼠标扫过窗口）

| 每帧 | 修复前 | 修复后 |
|---|---|---|
| `upload` 合计 | **18.0 ms** | **1.0–1.3 ms** |
| ├ mesh（顶点） | 12–16 ms | 0.8–1.0 ms |
| └ clip（遮罩） | 0.2–0.8 ms | 0.2–0.3 ms |
| `rebuild`（物理 + 变形 715 个 drawable） | 3.6–4.0 ms | 3.0–3.6 ms |
| 动画步数/帧 | 1.6–1.9（≈35 fps） | **1.00（60 fps）** |
| 帧时间 | 26–28 ms | **4.5–5 ms** |

- **正确性**：与未打补丁的构建做离屏像素 A/B（相同视线、相同 60 步 settle）：
  大肥鱼 **0 个像素不同**（max channel delta 0），Rice 同样 0。动态遮罩的更新路径与
  "整套重建"在像素上等价。
- **样例模型不受影响**：8 个官方样例逐帧 `upload` 0.01–0.11 ms，无错误。
- 表达式（会切换 art mesh 的路径，即 `from_drawables` 重建路径）跑了 6 个，无 issue。

### 兼容性说明（上游如果要采纳）

- `WgpuDrawableBuffers::vertex_buffer()` 返回类型变了（`&Buffer` → `BufferSlice`），这是
  不兼容改动；`draw.rs` 的三处调用点必须一起改（本补丁里已改）。
- `WgpuMeshUpdateError::{TextureIndex, BlendMode, Masks, InvertedMask}` 从此**不可达**。
  我们保留枚举变体以免破坏下游的 `match`，但更干净的做法是删掉并把新契约写进
  `update_drawables` 的文档（该函数目前没有文档注释）。
- 新增 `WgpuMeshUpdate::structure_changed()`；`bounds_changed`/`visibility_changed` 的语义不变。
- 还能再快一步（未做）：`renderer_vertex_data_changed` 与编码是两趟遍历，融合成
  一趟"边编码边比较"可以再省 30–40%（本 rig 0.9 ms → 估计 0.5 ms）。

---

## 5. 测试方法（都可以复现）

1. **像素 A/B**：`examples/offscreen` 输出 PNG，固定 `LIVE2D_LOOK_AT`、`LIVE2D_NO_MOTION`、
   固定 settle 步数；打补丁前后各渲一张，按通道逐像素比较。四个补丁都过了这一关。
2. **参数级**：`examples/sweep` 逐帧打印指定参数（`SWEEP_DIRECT`、`SWEEP_ALL`、
   `SWEEP_JUMP`），用来判断"是输入没到"还是"物理没传导"；`examples/jolt` 统计每帧变化像素数，
   用来找可见的跳变。
3. **成本分解**：`LIVE2D_PROFILE=1` 打一行每 60 帧的分项：

   ```
   live2d profile: 60 frames 1.00 steps, rebuild 3.4 ms, upload 1.1 ms (mesh 0.9 over 664 drawables, clip 0.2), delta 16.67..16.7 ms (mean 16.67, 0 zero, 0 under 1 ms)
   ```

   `steps` 是实际动画步数（>1 表示呈现跟不上）、`drawables` 是真正重传的 drawable 数。
   第 4 节的 18 ms 就是先靠这一行定位到 `upload`，再靠"静止模型也报 715 个变化"发现是重建路径。
4. **逐模型 smoke test**：8 个样例 × `offscreen` 各跑一遍，检查退出码、issue 输出、
   分项耗时是否在预期范围。

## 6. 复现所需资产

| 问题 | 能只用公开模型复现吗 |
|---|---|
| 1 负遮罩哨兵 | 不能。需要带 `-1` 哨兵的导出（我们的 rig 有 1 个）。判定方法：`masks` 里出现负数即可 |
| 2 无 mip 链 | 能。任意贴图远大于显示尺寸的模型都看得出来（8 个官方样例都行） |
| 3 物理链 | 能，但需要"setting 的输出被另一个 setting 当输入"的 rig；官方样例的物理链很短，效果不如我们的 131 组明显。判定方法：`physics3.json` 里某个 `destination.id` 同时出现在别的 setting 的 `source.id` |
| 4 逐 drawable 上传 | 成本能（任何 drawable 数上百的模型，`LIVE2D_PROFILE` 直接给出 upload 分项）；动态遮罩那部分需要"某个 drawable 的遮罩集合随参数变化"的模型 |

我们没有把 `DeepSeek 大肥鱼` 放进仓库（它不属于我们，也没有可再分发的许可）。如果你需要针对
动态遮罩的最小复现素材，可以在这份文档的 issue 下说明，我们再商量怎么提供（例如一个
只含几何与遮罩、不含贴图的裁剪版，或者一段能触发 `masks changed` 的参数扫描脚本）。

## 7. 给上游的建议

1. **优先级**：#3（物理链）和 #1（加载失败）是"错了但看得出来"；#2 是画质；#4 是性能，
   但对重 rig 是决定性的（35 fps → 60 fps）。
2. **可以拆成四个独立 PR**：它们互不依赖，改的文件也不同（`clipping.rs` /
   `pipeline.rs` / `core/physics.rs` / `buffers.rs` + `draw.rs`）。
3. **#4 的接口建议**：倾向于把"哪些字段会导致必须重建"写进 `update_drawables` 的文档注释，
   并把 `structure_changed` 作为返回值的一部分（本补丁的做法）；比让调用方从 `Err` 里猜更明确。
4. 我们很愿意把四个补丁整理成对应的 PR；如果你更希望只收其中一两个，也可以按需拆分。
