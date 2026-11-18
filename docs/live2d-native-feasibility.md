# 把 Live2D 集成进 Native App 的难度调研

> 调研时间：2026-11（依据 Cubism 5 SDK for Native R5 / Cubism 5.3 生态）
> 结论先行版见文末「TL;DR」

---

## 0. TL;DR

一句话：**"能跑"很容易，"能发版"是中等偏上，"用你自己的图形栈跑得漂亮"才难。**

- **容易的部分**：官方 SDK 已内置 5 个渲染后端（OpenGL / Metal / D3D9 / D3D11 / Vulkan），官方样品工程覆盖 Win/macOS/Linux/Android/iOS/HarmonyOS，跑通一个 Demo 通常是 **1～3 天**。
- **变难的第一处**：**你要的后端恰好不在官方列表里**（macOS 原生 AppKit 的 Metal、Android 的 Vulkan、D3D12、WebGPU、你自己的引擎）→ 必须自己实现 `CubismRenderer` 子类，含遮罩（clipping）、混合模式、离屏绘制，**6～12 周起步**，且 SDK 每次大版本升级都要跟着改。
- **变难的第二处**：真正的工程成本全在**周边**——资源/包体管线、GL 线程与生命周期、遮罩数量与显存预算、低端机帧率、透明窗口、热更新与分发。
- **最容易被低估的第三处**：**许可证 + Core 是闭源二进制**。个人/小规模（年营收 < 1000 万日元）免许可费，中大型要签 Publication License（一次性 ¥60 万 / ¥360 万 或 ¥40/份，最低 3 万份），且"可扩展应用"要提前送审、**发布前至少 1 个月**签约。
- **想最快出成果**：Unity（官方，0 成本）> UE（官方，R1 beta2）> Flutter/RN 社区插件（只覆盖 iOS/Android）> 直接改官方 Native 样品。

**按目标的推荐路线**

| 你的场景 | 推荐 | 预期 |
|---|---|---|
| 桌面玩具/桌宠/AI 陪伴，Win 为主 | Native C++ + D3D11 或 OpenGL，改官方样品 | 2～4 周出可用版 |
| iOS/Android 双端 App | iOS 用 Metal 后端、Android 用 GLES 后端（或 Java SDK） | 4～8 周 |
| Flutter/RN 已有 App，只想塞个形象 | `flutter_live2d` / `expo-live2d-cubism`（需自备 Core） | 1～5 天接完，1～2 周打磨 |
| macOS 原生（非 Catalyst） | 要么 OpenGL（已 deprecated），要么自写 Metal renderer | +3～6 周 |
| Godot | `gd_cubism`（GDExtension，非官方） | 1～3 天 |
| 自研引擎/特殊 GPU API | 自写 renderer | 3 个月量级 |
| 想完全绕开 Live2D 许可 | 纯 Rust 运行时 `mocari` + 自己的胶水（Rust + iced 桌面端） | **实测约 2 天跑通**动作/表情/物理/遮罩/视线跟随，见 [`rust-iced-live2d-plan.md`](rust-iced-live2d-plan.md)；`ayagami` 仍缺 motion/expression/pose |

---

## 1. 技术底座：你到底在集成什么

Cubism SDK 由两块拼成，理解这一点基本就理解了全部难度来源：

1. **Cubism Core（闭源二进制）**：负责 `.moc3` 的加载/校验与顶点变形计算。按协议**不能公开在 GitHub 上**，必须让使用者自己去官网下载并"注入"到工程里（这也是所有社区插件都要求你手动放 Core 的原因）。提供 DLL 与静态库两套，覆盖 Windows x86/x86_64、macOS arm64/x86_64、iOS arm64（含模拟器）、Android arm64-v8a / x86 / x86_64、Linux x86_64（ARM64、Raspberry Pi、UWP、Mac Catalyst 属 experimental）。
   参考：[Cubism Core 说明](https://docs.live2d.com/en/cubism-sdk-manual/cubism-core/)、[Core/README（库矩阵）](https://github.com/Live2D/CubismNativeSamples/blob/develop/Core/README.md)
2. **Cubism Native Framework（开源 C++）**：动作(motion3.json)、表情(exp3.json)、物理(physics3.json)、Pose、参数更新顺序、遮罩管理、渲染器抽象基类。

也就是说 **Core 给你"模型长什么样"，Framework 给你"怎么动"，渲染器给你"怎么画到你的屏幕上"**。Framework 是"只吃内存字节流"的设计——文件 IO（Android assets / iOS bundle / 你自己的包体）**要你自己实现**，官方样品里那段 `LAppPal::LoadFileAsBytes` 是给样品用的，不能直接搬。

### 渲染器抽象是关键的"分水岭"

Framework 里每个图形 API 对应一个 `CubismRenderer_*`（如 `CubismRenderer_OpenGLES2`、`CubismRenderer_Metal`、`CubismRenderer_Vulkan`）。每个 drawable 的绘制要自己处理：

- 按 `renderOrder` 排序（Native 侧**只支持 draw order 升序**，没有 Unity 那种 Z 排序/中断绘制）；
- 每个 ArtMesh 一次 draw call（模型精细时 draw call 数会很可观）；
- **遮罩 = 预先合成的 mask texture**，默认上限 **36 个**，超过就要开"高精度遮罩"（性能换质量）；
- **混合模式 / 离屏绘制是 Cubism 5.3 新增能力**，Framework 里 Vulkan 与 Metal 后端是分版本才补齐的（Vulkan 的 blend/offscreen 在 R5 beta3 才加）。→ 你自写的 renderer 会是**长期维护负担**，不是一次性投入。

参考：[Cubism SDK 对比（遮罩/绘制顺序/上限）](https://docs.live2d.com/en/cubism-sdk-manual/cubismsdk-compare/)、[遮罩预处理方法（Native）](https://docs.live2d.com/en/cubism-sdk-manual/ow-sdk-mask-premake/)、[Framework 源码](https://github.com/Live2D/CubismNativeFramework)

---

## 2. 官方支持矩阵（截至 2026-09-29 更新）

**Cubism SDK for Native 渲染后端 × 平台**

| 后端 | Windows | macOS | Linux* | Android | iOS* | HarmonyOS |
|---|---|---|---|---|---|---|
| OpenGL | ✓ | ✓ | ✓ | ✓ | ✓ | ✓ |
| Metal | – | ✓（仅 Mac Catalyst） | – | – | ✓ | – |
| DirectX 9.0c | ✓ | – | – | – | – | – |
| DirectX 11 | ✓ | – | – | – | – | – |
| Vulkan | ✓ | – | ✓（Ubuntu 18.04） | – | – | – |
| Cocos2d-x | ✓ | ✓ | ✓ | ✓ | ✓ | – |

\* 仅 64 位；Core 侧 **Android 没有 armeabi-v7a**，iOS 没有 armv7。Cocos2d-x 支持已在 Native R2 后终止。
来源：[平台支持状况](https://docs.live2d.com/en/cubism-sdk-manual/platform/)

**这张表就是要命的三个缺口：**

1. **macOS 原生 App（AppKit/SwiftUI）没有 Metal 后端** —— 官方只给了 Mac Catalyst 版。不想上 Catalyst，就只能用 OpenGL（自 10.14 deprecated；macOS 26 移除了 AGL，但 OpenGL.framework 仍随 SDK 提供，Apple 未宣布移除日期）或自己写 Metal renderer。参考：[Apple 论坛：OpenGL 是否会在 26 后移除](https://developer.apple.com/forums/thread/796543)、[macOS 26 Release Notes（AGL 移除，OpenGL 保留）](https://developer.apple.com/documentation/macos-release-notes/macos-26-release-notes)
2. **Android 没有 Vulkan 后端** —— 如果你的 App 主渲染管线是 Vulkan（现代 Android 引擎常见），必须桥接 GL 上下文或自写 Vulkan renderer。
3. **图形栈严重偏 GLES2 世代** —— iOS 的 GLES 同样早已 deprecated。短期没问题（官方样品与所有社区插件都跑在 GLES 上），但这是明确的长期技术债。

**引擎内路线（难度完全不同）**

| 方案 | 状态 | 备注 |
|---|---|---|
| Unity | 官方，成熟 | 支持 Win/mac/Linux/Android/iOS/WebGL/HarmonyOS(团结引擎)/PS/Switch，URP/BiRP 都有方案 |
| Unreal Engine | **官方，Cubism 5 SDK R1 beta2（2026-09-29）** | 需 UE 5.3+，作为插件分发 |
| Godot | 非官方 `gd_cubism`（GDExtension，Godot 4.3+） | 桌面成熟，iOS/Android 标注 experimental |
| Cocos Creator | 官方 JS 版 | — |
| Java SDK | 官方 R5（2026-06） | **Android 可以纯 Java 接入，避开 NDK/JNI** |

参考：[SDK 手动目录与版本公告](https://docs.live2d.com/en/cubism-sdk-manual/top/)、[UE 接入教程](https://docs.live2d.com/en/cubism-sdk-tutorials/getting-started-ue/)、[gd_cubism](https://github.com/mizunagikb/gd_cubism)

---

## 3. 难度分档（含工作量估算）

> 人日 = 1 名熟悉该平台、但**没做过 Live2D** 的中高级工程师，含调试时间，不含美术/建模。

| 档 | 场景 | 难度 | 首帧 | 可发布 |
|---|---|---|---|---|
| **0** | Unity / UE 内使用 | ★☆☆☆☆ | 半天 | 1～3 天 |
| **1** | 改官方样品：Win(D3D11/GL)、iOS(Metal)、Android(GLES)、Linux(GL/Vulkan) | ★★☆☆☆ | 1～3 天 | 2～4 周 |
| **2** | 桥接层：Flutter / RN / Godot / WebView 壳 | ★★☆☆☆～★★★☆☆ | 1～5 天 | 1～4 周（受插件能力限制） |
| **3** | 自家 App 内嵌：自管 GL 上下文/线程/资源/包体（无官方样品可抄） | ★★★☆☆ | 1～2 周 | 4～8 周 |
| **4** | macOS 原生 Metal、Android Vulkan、D3D12、WebGPU（自写 renderer） | ★★★★☆ | 2～4 周 | 2～3 个月，且长期维护 |
| **5** | 绕开 Live2D 自研运行时（纯 Rust 重放 moc3） | ★★★★★ | 数周（只能画静态） | 已可用（见下） |

**为什么"档 3"比想象中贵？** 官方样品是"一个窗口 + 一个模型 + 一个 GL 循环"的玩具结构；换成真实 App 后要面对：一个进程多个视图/多次进出后台、渲染线程与 UI 线程分离、模型资源随场景加载卸载、Core 的 `csmReviveMocInPlace` 需要**手动分配对齐内存**且失败时只回 NULL、模型文件版本必须 ≤ Core 支持版本（否则直接加载失败）。

---

## 4. 真实踩坑清单（按层次，这部分是"难度"的实际内容）

### 4.1 构建 / 链接
- iOS 的 Core 是**静态库**（`libLive2DCubismCore.a`），模拟器 arm64 / 模拟器 x86_64 / 真机 arm64 各一份，需要用 xcframework 或 build phase 区分；官方 NOTICE 承认 **ios-cmake 4.5.0 在真机 ARM 上会装不上**，需要回退 4.4.1 或手工改签名设置。
- Mac Catalyst + Metal：**CMake ≥ 3.27 会报 `NSURL not found`**，必须用 ≤ 3.26。
- Linux：GLEW 2.2.0 不支持 CMake 4.0；**Wayland 不支持**（EGL 下 glewInit 异常），只能 X11。
- 来源：[Cubism Native Samples NOTICE.md](https://github.com/Live2D/CubismNativeSamples/blob/develop/NOTICE.md)
- Android：`UnsatisfiedLinkError: libLive2DCubismCore.so not found` 是最常见的第一道坎，需要用 `readelf -d` 确认 `NEEDED` 里真的有 Core；16KB page size（Android 15+ 上架要求）需要自己验证 .so 对齐。
- 社区实战记录（中文，含排错顺序）：[Android 移植：NDK/CMake/JNI/白屏排错](https://blog.csdn.net/weixin_31534091/article/details/169832235)、[iOS 接入流程](https://blog.csdn.net/Android_yh/article/details/130140988)

### 4.2 渲染线程与生命周期
- Android：`GLSurfaceView` 默认 `RENDERMODE_WHEN_DIRTY`，忘记 `setRenderMode(RENDERMODE_CONTINUOUSLY)` 的表现就是"闪一下就静止"，很容易被误判成加载失败。
- `AAssetManager_fromJava` 只能在有效 JNIEnv 线程上取一次，而 GL 线程 ≠ UI 线程 → 必须"进一次、缓存住"。
- iOS：`CAEAGLLayer.isOpaque = false` + `EAGLContext` 单队列串行；多视图共享一个 context 时要自己序列化。

### 4.3 资源与纹理
- 贴图普遍非 2 次幂：GLES2 下必须 `GL_CLAMP_TO_EDGE`，不能生成 mipmap，否则**模型轮廓和物理都在动、但完全没有颜色**（非常具有欺骗性）。
- 贴图通道数（RGBA/RGB/灰度）写死会上传错。
- 显存预算：单个模型贴图 2048×2048×RGBA8 约 16 MB，多模型 + 遮罩 render target 很容易 OOM；社区项目给出的经验上限是单图最长边 ≤2048、总量 ≤128 MiB、遮罩 RT ≤1024（手动最大 2048）。
- 参考：[STS2 Live2D 故障排查（含显存/遮罩上限经验值）](https://lingkongsky.github.io/STS2-Live2D/guide/troubleshooting)

### 4.4 模型与 Core 版本
- moc3 有版本号（3.0/3.3/4.0/4.2/5.0…），**Core 只能读 ≤ 自己版本的 moc3**；用旧 Core 打开新模型，`csmReviveMocInPlace` 返回 NULL，只会往日志里吐一行 "Core unsupport later than moc3 ver"。
- 结论：**Core 版本要钉住并纳入升级流程**，否则美术换新版本 Editor 导出后，线上老用户直接白屏。
- 参考：[Cubism Core API Reference（moc3 版本枚举）](https://cubism.live2d.com/sdk-doc/reference/NativeCoreAPIReference_en_r13.pdf)

### 4.5 遮罩 / 混合 / 离屏
- 默认 36 个遮罩上限，超了就要开高精度遮罩（显存/带宽换正确性）；大 ArtMesh 被小遮罩裁切时，遮罩区域"被压扁"导致渲染错误是官方承认的已知现象，只能加大遮罩 buffer。
- 5.3 新增的 blend mode / offscreen drawing 在自写 renderer 上要**额外实现**。

### 4.6 平台窗口/透明（桌宠类需求的隐藏成本）
- 透明窗口 + 点击穿透在 Windows（WS_EX_LAYERED / DirectComposition）、macOS（NSWindow + Metal，或 GL 已 deprecated）、Linux（X11 composite）各不相同；
- 这不是 Live2D 的问题，但"Live2D 桌宠"类产品 80% 的坑在这里。

### 4.7 内容与合规
- Apple 隐私清单：官方 NOTICE 明确"本产品不使用受隐私清单约束的 API"——但你自己的工程要自行申报。
- moc3 是二进制，**有被篡改导致 Editor/Core 崩溃的历史**，官方提供 MOC3 Consistency Checker；做 UGC 模型加载的产品必须自己加校验。

---

## 5. 许可证与法务（Native 发行的硬门槛，别等做完才发现）

| 项 | 事实 |
|---|---|
| 开发阶段 | 免费（同意 Proprietary + Open Software 两份协议即可下载） |
| 发行阶段 | 需要 **SDK Release License（Publication License Agreement）**；个人与**年营收 < 1000 万日元**的小规模主体**免签约、免付费** |
| 中规模（< 1 亿日元） | 一次性 ¥600,000，或 ¥40/份（无最低量） |
| 大规模（> 1 亿日元） | 一次性 ¥3,600,000，或 ¥40/份（**最低保底 30,000 份**） |
| 平台/地区 | 一次性费用**不按平台/地区重复收**（iOS/Android/Web 同款内容不再加钱） |
| 时效 | 合同必须**至少提前 1 个月**完成 |
| "Expandable Application" | 有"显著可扩展性"的应用属于特殊类别，**需另行送审 + 签特殊协议**。官方明确把"VTuber 直播用的追踪软件"归入此类；"可加载任意用户模型"的桌宠/编辑器类需按官方流程图自行判定，倾向保守处理 |
| VTuber 经营 | 年收入超 **2000 万日元**且符合条件时另有许可要求 |
| Cubism Core | 闭源，**不得公开分发/开源**；使用者需自行从官网获取。这也是所有开源项目只能"留个空位"的原因 |

来源：[SDK Release License](https://www.live2d.com/en/sdk/license/)、[一次性买断方案价格表](https://www.live2d.com/en/sdk/license/purchase_plan01/)、[Expandable Applications](https://www.live2d.com/en/sdk/license/expandable/)、[Copyright Simple License Plan](https://www.live2d.com/en/business/slp/)

**⚠️ 最容易踩的坑**：做一个"用户可以自己导入模型"的桌宠/AI 陪伴 App → 大概率被判定为 Expandable Application，需要提前送审，**审不过来就发不了版**。纯"内置固定模型"则按普通非营利/买断/分成方案走，简单得多。

---

## 6. 现成方案横评（别重复造轮子）

| 方案 | 平台 | 成熟度 | 能力边界 | 备注 |
|---|---|---|---|---|
| **Unity 官方 SDK** | 全平台 | 高 | 全功能（含 URP 遮罩、自定义渲染 pass） | 包体大；不是"原生 App"体验 |
| **UE 官方 SDK** | Win/mac/Android/iOS | beta2 | 插件形态，功能追平中 | UE 5.3+ |
| **Native 官方样品** | 全平台 | 高 | 全功能，无 UI 封装 | 你的主力起点 |
| **`gd_cubism`（Godot）** | 桌面成熟 / 移动 experimental | 中 | 支持 motion/expression/physics/pose、自定义 shader | 0.9 起改为直接绘制，省显存 |
| **`flutter_live2d`** | **仅 Android / iOS** | 中（1.0.2，2026-05） | 透明背景、多视图、motion/expression/parameter、触摸跟踪、暂停 | **桌面不支持**；自带 Core（注意许可） |
| **`expo-live2d-cubism`** | Android / iOS（Expo dev client） | 中（1.0.4，约 8 个月前） | 口型同步、motion/exp3、陀螺仪 | **不支持 Expo Go**，需手动注入 Core |
| **Ayagami（Rust）** | wgpu 覆盖面 | 低-中 | 能解析 moc3 到 SDK5.0 特性、渲染质量高 | **motion/expression/physics 仍在 TODO** |
| **Mocari（Rust）** | wgpu 可选后端 | 低 | moc3 读取、渲染、motion、expression、physics、pose、遮罩（含反转遮罩） | **本仓库已实测可用**：Windows 桌面 + iced 0.15-dev + wgpu 30，131 组物理 setting 跑通，与官方 Viewer 逐项统计一致；需打 2 个补丁（负 mask 哨兵、mipmap）。详见 [`rust-iced-live2d-plan.md`](rust-iced-live2d-plan.md) |
| **WebView + pixi-live2d-display** | 全平台壳 | 高（Web 侧） | 生态最全 | 性能/透明/输入代理都是新坑；适合 Tauri/Electron 壳 |

来源：[flutter_live2d](https://pub.dev/packages/flutter_live2d)、[expo-live2d-cubism](https://www.npmjs.com/package/expo-live2d-cubism)、[gd_cubism](https://github.com/mizunagikb/gd_cubism)、[Ayagami](https://github.com/AyagamiDev/ayagami)、[Mocari](https://github.com/Eatgrapes/Mocari)

---

## 7. 建议的验证路径（90 分钟 spike，花最小的钱证伪）

1. **先定图形后端**：你要接的 App/引擎现在用什么 API？→ 直接命中第 2 节表格里 ✓ 的，选"档 1"；不命中就是"档 4"，先把工作量估到 1 个月以上。
2. **下载 `CubismSdkForNative-5-r.5`**，把 `Core/` 拷进 `CubismNativeSamples/Core/`（**只换 Core，不要动 Framework**——这一步搞错会缺文件跑不起来，社区里最常见的错误）。
3. **跑官方对应后端的 Demo**（Win: D3D11/OpenGL；iOS: Metal；Android: OpenGL）。能出模型 = 环境通关。
4. **再花 1 天做"嵌入验证"**：把样品里的 `LoadFileAsBytes` / `LAppPal` 换成你自己 App 的资源读取，把"窗口+GL 循环"换成你自己 App 的渲染循环。**这一步能不能过，才是真正的可行性判断**——90% 的"卡住"都发生在这里，而不是 SDK 本身。
5. 再做 1 个**性能基线**：满屏 1080p、目标 60fps、2 个模型、遮罩 36 上限打满，记下 draw call 与 GPU 时间。

---

## 8. 结论

- **难度评级：★★★☆☆（中等偏上）**，与"接一个 3D 骨骼动画库 + 自己搞定资源管线和遮罩"相当。
- **不是"接 SDK"，而是"接一套运行时 + 一套资源管线 + 一条渲染线程"**。
- 最大的三个不确定性，按概率排序：
  1. **图形后端不匹配**（macOS 原生 / Android Vulkan / D3D12）——决定是 2 周还是 3 个月；
  2. **许可证类别**（尤其"可加载用户模型"→ Expandable Application 送审）；
  3. **内容侧**（模型资源体积/显存/低端机帧率）而非代码。
- 如果只是想在 App 里"有个会动的角色"，**做 Flutter/RN/Unity 路线，不要碰 Native C++**；如果要做桌宠/陪伴类常驻 App（透明窗口、低占用、开机自启），**Native 才是正解，且这恰恰是官方样品最不覆盖的部分**。

---

## 参考资料

- Live2D 官方手册：[平台支持状况](https://docs.live2d.com/en/cubism-sdk-manual/platform/)｜[SDK 对比](https://docs.live2d.com/en/cubism-sdk-manual/cubismsdk-compare/)｜[Cubism Core](https://docs.live2d.com/en/cubism-sdk-manual/cubism-core/)｜[FAQ](https://docs.live2d.com/en/cubism-sdk-manual/faq/)｜[遮罩预处理（Native）](https://docs.live2d.com/en/cubism-sdk-manual/ow-sdk-mask-premake/)｜[版本公告索引](https://docs.live2d.com/en/cubism-sdk-manual/top/)
- 源码与样品：[CubismNativeSamples](https://github.com/Live2D/CubismNativeSamples)｜[NOTICE（已知问题）](https://github.com/Live2D/CubismNativeSamples/blob/develop/NOTICE.md)｜[Core 库矩阵](https://github.com/Live2D/CubismNativeSamples/blob/develop/Core/README.md)｜[CubismNativeFramework](https://github.com/Live2D/CubismNativeFramework)
- 许可：[SDK Release License](https://www.live2d.com/en/sdk/license/)｜[买断价目表](https://www.live2d.com/en/sdk/license/purchase_plan01/)｜[Expandable Applications](https://www.live2d.com/en/sdk/license/expandable/)｜[Simple License Plan](https://www.live2d.com/en/business/slp/)
- 生态：[flutter_live2d](https://pub.dev/packages/flutter_live2d)｜[expo-live2d-cubism](https://www.npmjs.com/package/expo-live2d-cubism)｜[gd_cubism](https://github.com/mizunagikb/gd_cubism)｜[Ayagami](https://github.com/AyagamiDev/ayagami)｜[Mocari](https://github.com/Eatgrapes/Mocari)
- 实战排错：[Android NDK/JNI/白屏](https://blog.csdn.net/weixin_31534091/article/details/169832235)｜[Android GLES2+JNI 全链路](https://blog.csdn.net/weixin_29170327/article/details/170209649)｜[iOS 接入](https://blog.csdn.net/Android_yh/article/details/130140988)｜[显存/遮罩上限经验](https://lingkongsky.github.io/STS2-Live2D/guide/troubleshooting)｜[HarmonyOS 接入经验](https://developer.huawei.com.cn/consumer/cn/forum/topic/0213216784934308034)
- 平台侧：[Apple 论坛：OpenGL 是否移除](https://developer.apple.com/forums/thread/796543)｜[macOS 26 Release Notes](https://developer.apple.com/documentation/macos-release-notes/macos-26-release-notes)
