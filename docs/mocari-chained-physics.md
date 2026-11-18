# 已确认缺陷：mocari 0.4.1 的物理设置之间不传导

**状态：已定位到具体代码行，已打补丁并验证。** 补丁见
[`../patches/mocari-0.4.1-chained-physics.patch`](../patches/mocari-0.4.1-chained-physics.patch)。

- 版本：`mocari 0.4.1`（`.refs/mocari`，另含本仓库的负遮罩、mipmap 两个补丁，与本问题无关）
- 涉及文件：`src/core/physics.rs`（`PhysicsRuntime::evaluate`、`evaluate_setting`）
- 对照实现：`CubismNativeFramework/src/Physics/CubismPhysics.cpp`
  （`git clone --depth 1 https://github.com/Live2D/CubismNativeFramework .refs/cubism-framework`）
- 模型：`DeepSeek 大肥鱼`，131 组 physics setting / 299 输入 / 292 输出

---

## 1. 现象

物理输出**不会**作为后续 setting 的输入被看到。链路 `A → 中间参数 → 次级部件` 里，中间参数
是物理输出时，次级部件完全不动。

`examples/sweep.rs` 以 60 fps 把 `AngleX`（追踪输入）从 -1 扫到 +1：

```
frame     x   AngleX  ParamAngleX  ParamBodyAngleX  HipAngleX  HipZ1  FrontHairX1
    0 -1.000  -30.000     -28.002           0.000     0.000 -0.000       0.000
  239  1.000   30.000      27.750           0.000     0.000 -0.000       0.000
```

`ParamAngleX` 已经到 ±27.75，但 `FrontHairX1` 恒为 0。而 `physics3.json` 里
`PhysicsSetting50` 就是 `ParamAngleX (X, weight=100)` → `FrontHairX1..X4`（scale 1, weight 100），
按定义它必须跟着 `ParamAngleX` 走。

同一次 sweep 里，**写入**的参数是能被物理看到的：直接写 `ParamAngleX`（`SWEEP_DIRECT=ParamAngleX`）
时 `FrontHairX1` 出现过 -0.737。也就是说：app 写进参数的值进得了物理输入，物理自己算出来的值进不了。

## 2. 根因

`evaluate_setting` 在写回输出参数时，把**重新绑定后的缓存引用**当成了计算结果：

```rust
let value = match output.kind { /* 振子算出来的值 */ };
setting.current_outputs[index] = value;

let Some((value, (&minimum, &maximum))) = parameter_cache.get_mut(parameter_index).zip(…) else { … };
//        ^^^^^ 这里遮蔽了上面的 value，变成 &mut f32
update_output_parameter(value, minimum, maximum, *value, output);
//                                                ^^^^^^ 传的是缓存当前值
```

于是这次写入等价于 `cache = blend(cache, cache)`，是一个**空操作**。振子结果只存在于
`current_outputs`，帧末由 `Interpolate` 写进 `parameter_values`（所以画面上的
`ParamAngleX` 是对的），但**永远进不了 `parameter_cache`**。下一个 setting 读的是缓存，
看到的是 `parameter_values` 在子步开始时的值（通常就是默认值 0）。

官方的写入目标是缓存，而且注释点名了这就是链路传导机制：

```cpp
// _parameterCachesはグループ間での値の伝搬の役割があるので
// _parameterInputCachesとの分離が必要。
// _parameterCaches needs to be separated from _parameterInputCaches because of
// its role in propagating values between groups.

UpdateOutputParameterValue(
    &_parameterCaches[currentOutputs[i].DestinationParameterIndex],   // 写缓存
    …,
    outputValue,                                                     // 传计算值
    &currentOutputs[i]);
```

## 3. 三个观测互相印证

| 观测 | 补丁前 | 补丁后 | 说明 |
|---|---|---|---|
| 驱动 `AngleX`（正常路径）→ `FrontHairX1` | 0.000 | **-0.136** | 输出现在能进缓存，链路通了 |
| 直接写 `ParamAngleX` → `FrontHairX1` | -0.737 | **0.000** | 补丁前那条路径“能用”恰恰是因为写入是空操作，缓存保住了我们写的值；补丁后 `PhysicsSetting1` 会正确地把它覆盖成它自己的输出 |
| 直接写 `ParamAngleX` → 读回 `ParamAngleX` | 0 | 0 | 预期行为，官方也一样：`Setting1` 的输出就是 `ParamAngleX` |

第二行是根因的直接证据：**同一个参数，改成“由物理产出”就传不下去，改成“由 app 写入”就能传下去**
——差别只在于缓存有没有被写。

## 4. 影响面

本模型 **113 处**跨 setting 读取（`AngleX → ParamAngleX → 前发/侧发/后发`、
`ParamBodyAngleX → 十余处`、`ParamBreath → 胸腔/裙摆/缎带` …）。打补丁前，**这 113 条链路全部是断的**，
只有“app 直接写入的参数”能驱动物理。观感上就是头发/裙摆的二次联动明显偏死，而且很容易被误判成
“mocari 的物理就是偏死”。

诊断脚本（本地一次性核对用）：按 `physics3.json` 统计 `Output.Destination.Id` 被其它 `Input.Source.Id`
引用的情况，即可算出跨 setting 读取数。

## 5. 还没解决的部分

**中线附近的“顿一下”不是这个缺陷。** 打完补丁后：

- `examples/sweep.rs` 在 ±0.12 范围、240 帧慢扫，509 个参数**没有任何一帧的跳变超过 0.05**；
- `examples/jolt.rs`（逐帧比较渲染结果的像素差）在 ±1 全程、120 帧下，每帧变化像素数的
  **峰值只有中位数的 1.3 倍**，没有尖峰；
- 同一批测量显示，注视只让**头部/头发**动，身体（画面下半部）在整个 ±1 扫动里位移恒为 0。
  （这条后来查清了：不是缺陷，是当时没喂 `PositionX`。模型的 `*.vtube.json` 把
  `FaceAngleX → PositionX` 映射为身体输入，补上之后躯干和手臂会跟着转；见
  `rust-iced-live2d-plan.md` 的“鼠标该绑哪些参数”。）

所以“鼠标过中线时身体顿一下”如果确实存在，它不在参数层面，也不在离线渲染层面；需要在
**交互路径**上测（真实帧时间抖动、prepare 调用频率）。下一步是在 app 里加一条
`LIVE2D_TRACE` 逐帧日志（dt / gaze / 被驱动参数），复现时抓一段数据再判断。

## 附：其它已确认的 mocari 缺陷（均已打补丁）

| 缺陷 | 症状 | 补丁 |
|---|---|---|
| 负遮罩索引 `-1` 未过滤 | 整模型加载失败 `invalid mask drawable index -1` | `mocari-0.4.1-negative-masks.patch` |
| 贴图无 mipmap | 缩小后细线稿变虚线/麻点 | `mocari-0.4.1-mipmaps.patch` |
| 物理输出不写回参数缓存 | 多级物理链路全断，二次联动偏死 | `mocari-0.4.1-chained-physics.patch` |

三条都可以直接提 upstream。
