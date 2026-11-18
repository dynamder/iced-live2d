# 疑似缺陷：mocari 0.4.1 的物理设置之间不传导（chained physics）

**状态：未定论。** 有可复现的观测，但根因还没定位到具体代码行，所以先记录，不当作结论用。

- 版本：`mocari 0.4.1`（`.refs/mocari`，含本仓库两个 patch：负遮罩、mipmap；两者都与此问题无关）
- 涉及文件：`src/core/physics.rs`（`PhysicsRuntime::evaluate`、`evaluate_setting`）、`src/runtime.rs`（`apply_physics`）
- 模型：`DeepSeek 大肥鱼`（131 组 physics setting / 299 输入 / 292 输出）

---

## 1. 现象

物理输出**不会**作为后续 setting 的输入被看到。表现为：链路 A → 中间参数 → 次级部件，中间参数是物理输出时，次级部件不动。

用 `examples/sweep.rs` 以 60 fps 从 `-1` 扫到 `+1`，三种驱动方式对比：

### (a) 走追踪输入 `AngleX`（官方的正常路径）

```
frame     x   AngleX  ParamAngleX  ParamBodyAngleX  HipAngleX  HipZ1  FrontHairX1
    0 -1.000  -30.000     -28.002           0.000     0.000 -0.000       0.000
   60 -0.498  -14.937     -14.194           0.000     0.000 -0.000       0.000
  120  0.004    0.126      -0.135           0.000     0.000 -0.000       0.000
  180  0.506   15.188      13.925           0.000     0.000 -0.000       0.000
  239  1.000   30.000      27.750           0.000     0.000 -0.000       0.000
```

`ParamAngleX` 已经到 ±27.75，但 `FrontHairX1` 恒为 0。

而 `physics3.json` 里 `PhysicsSetting50` 就是：

```
inputs:  ParamAngleX (type=X, weight=100) | ParamAngleZ (type=Angle, weight=52)
outputs: FrontHairX1 / X2 / X3 / X4 (scale=1, weight=100)
```

按定义 `FrontHairX1` 应当跟随 `ParamAngleX`。

### (b) 直接把 `ParamAngleX` 当输入写（`SWEEP_DIRECT=ParamAngleX`）

```
frame     x   AngleX  ParamAngleX  ParamBodyAngleX  HipAngleX  HipZ1  FrontHairX1
  120  0.004    0.000       0.000           0.000     0.000 -0.000      -0.737
  180  0.506    0.000       0.000           0.000     0.000 -0.000      -0.065
```

`ParamAngleX` 读回来是 0（这是**预期**的：`PhysicsSetting1` 的 `AngleX → ParamAngleX` 会把它覆盖回 0，官方也一样），但 **`FrontHairX1` 出现了非零值**。

### (c) 关键对比

同样是 `ParamAngleX` 这个量：

| 场景 | 输入缓存里的 `ParamAngleX` | `FrontHairX1` 读数 |
|---|---|---|
| (a) `ParamAngleX` 由 physics 产出 | 物理结果（27.75） | **0** |
| (b) `ParamAngleX` 由我们直接写入 | 我们写的值 | **非 0（-0.737）** |

也就是说：**写到参数上的值能进物理输入，而物理自己产出的值进不去。** 次级 setting 只能看到“app 写进来的值”，看不到“上一级 physics 算出来的值”。

---

## 2. 为什么这不像“模型数据问题”

- `PhysicsSetting50` 的输入/输出/权重都是正常的（见上），`Weight=100`、`Scale=1`；
- 官方 SDK 的 `CubismPhysics::Evaluate` 会把每个 setting 的输出**写回 `_parameterCache`**，所以后面的 setting 读同一个缓存就能拿到前一级的结果 —— 这是 Cubism 多级物理（`AngleX → ParamAngleX → 头发`、`ParamBreath → 胸腔 → 裙摆`）能成立的前提；
- 本模型是这种多级结构的典型：131 组 setting 里大量 “A 的输出是 B 的输入”。

---

## 3. 两个候选根因（都还没验证）

**候选 1：`evaluate_setting` 的输出没写进 `parameter_cache`，而是写去了别处。**

`PhysicsRuntime::evaluate` 的子步循环大致是：

```rust
while self.current_remain_time >= physics_delta_time {
    // 1) 用 input_cache 和当前 parameter_values 平滑出 parameter_cache
    for (index, &parameter_value) in parameter_values.iter().enumerate() {
        self.parameter_cache[index] =
            self.parameter_input_cache[index] * (1.0 - input_weight)
                + parameter_value * input_weight;
        self.parameter_input_cache[index] = self.parameter_cache[index];
    }

    // 2) 逐 setting 求值（注意它拿的是 &mut parameter_cache）
    for setting in &mut self.settings {
        evaluate_setting(setting, &mut self.parameter_cache, ...);
    }

    self.current_remain_time -= physics_delta_time;
}

// 3) 最后才把 previous/current 插值写回 parameter_values
```

如果 `evaluate_setting` 只改 `parameter_values`（或只改自己的状态）而不改 `parameter_cache`，那么步骤 2 里后面的 setting 读到的就是步骤 1 刚写的“平滑值”，永远看不到前一级输出 → 完全符合观测 (a)。

**候选 2：每帧 `parameter_values` 被重置，导致链路的“上游值”每帧都是默认值。**

我们的调用顺序是 `reset_parameters()` → 应用 motion/override → `apply_physics()` → `update_meshes()`。物理的输出在**帧末**才写进 `parameter_values`，而下一帧开头 `reset_parameters()` 又把它清掉。如果 `parameter_cache` 在每帧第一批子步里从 `parameter_values` 重建，那么上游输出永远只在“同一帧的同一批子步内”短暂存在——能不能被下游看到，取决于遍历顺序，非常脆弱。

（官方实现里 `_parameterCache` 是**跨帧保留**并在子步里平滑更新，不存在这个问题。）

---

## 4. 怎么确认

按成本排序：

1. **在 `evaluate_setting` 里打日志**：对 `PhysicsSetting1` 和 `PhysicsSetting50`，打印它们读到的 `parameter_cache[ParamAngleX]` 与写出的值。如果 50 读到的永远是 0 → 候选 1 成立。
2. **对比缓存与值**：在子步循环结束后、写回 `parameter_values` 之前，dump 一次 `parameter_cache[ParamAngleX]`。
3. **官方 Viewer 目视对照**：把 `AngleX` 滑块拉到 ±30（本模型的默认绑定点），看**前发/侧发**是否明显摆动。如果官方明显摆、我们不动 → 观测成立。
4. 补一个单元测试：构造两个 setting 的最小 physics3（A→中间参数，中间参数→出参），断言出参非零。这个最适合直接提 upstream。

---

## 5. 影响面（如果成立）

- 本模型 **131 组 setting 里大部分是多级的**，包括 `ParamBreath → 胸腔/裙摆/缎带`、`ParamAngleX → 前发/侧发/后发`、`HipAngle* → 腿部/饰品`；
- 结论会是：**只有“app 直接写入的参数”能驱动物理，物理自己算出来的参数驱动不了下一级**；
- 观感上就是“头发/裙摆的联动明显比官方弱”“身体没有层次感”，但因为我们一直在调别的 bug，很容易误判成“mocari 物理就是偏死”。

注意一条**不能**作为证据的现象：`ParamBodyAngleX` 读回 0 是**预期行为**，不是缺陷。它的生产者是 `PhysicsSetting4: PositionX → ParamBodyAngleX`，而官方默认绑定里 `PositionX` 是“未设置”（等于 0），官方 Viewer 同样会算出 0。

---

## 6. 临时规避

在根因确认前，如果只是想让观感接近官方，可以：

- 直接把**中间参数**当作输入写（例如同时写 `ParamAngleX` 与 `AngleX`）——但这会偏离官方绑定表，导致无法与参考渲染器逐像素对比，**不建议长期用**；
- 或者 patch `physics.rs`：让 `evaluate_setting` 把输出写回 `parameter_cache`，并让 `parameter_cache` 跨帧保留。

---

## 附：其它已确认的 mocari 缺陷（已修，见 `patches/`）

| 缺陷 | 症状 | 补丁 |
|---|---|---|
| 负遮罩索引 `-1` 未过滤 | 整模型加载失败 `invalid mask drawable index -1` | `mocari-0.4.1-negative-masks.patch` |
| 贴图无 mipmap | 缩小后细线稿变虚线/麻点 | `mocari-0.4.1-mipmaps.patch` |

这两个可以直接提 upstream；上面这条链式传导问题建议先把第 3 节第 4 条的单元测试写出来再提。
