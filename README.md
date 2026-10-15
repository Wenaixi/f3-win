# F3-Win

F3-Win 是 Linux 闪存测试工具 F3 (Fight Flash Fraud) 的 Windows 图形界面版本。支持无缓存直写、快速容量探测与分区修复。

---

## 主要功能

### 1. 写入测试 (f3write) 与读取校验 (f3read)
- 在磁盘空闲空间中写入标准 1GB 大小的 `.h2w` 数据文件；
- 使用 Win32 Direct I/O (`FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`) 绕过系统缓存，测得闪存颗粒的真实写入速度；
- 读回数据并逐扇区比对，统计正常、损坏、异常与被覆盖扇区，生成健康分布图；
- 支持校验完成后自动删除测试文件，释放磁盘空间。

### 2. 容量探测 (f3probe)
- 直接对物理磁盘写入跳跃扇区探针，在 1 到 3 分钟内测出扩容盘的真实物理容量；
- 在测试前冲刷主控内部缓存，防止控制器 RAM 伪造读取数据；
- 注意：该测试直接操作物理扇区，测试前会自动卸载卷，请对已备份或空盘使用。

### 3. 分区修复 (f3fix)
- 根据探测出的实际物理容量计算安全边界，并生成 Windows `diskpart` 脚本；
- 支持直接调用系统 `diskpart.exe` 重新创建合规主分区并格式化。

---

## 运行与编译

### 运行需求
- Windows 10 / 11 (x64 或 ARM64)；
- 直接双击 `f3-win.exe` 运行。程序会申请管理员权限以支持底层磁盘探测与卷卸载。

### 编译
本项目通过 GitHub Actions 自动化编译发布：
- 推送代码到 `main` 分支自动触发 CI 构建；
- 产出产物包含 x64 与 ARM64 单文件可执行程序压缩包。

---

## 目录结构

```text
f3-win/
├── .github/workflows/ci.yml       # CI 编译流水线
├── Cargo.toml                     # 依赖与编译配置
├── build.rs                       # UAC 权限清单配置
├── src/
│   ├── main.rs                    # 程序入口与提权逻辑
│   ├── core/                      # 算法与底层驱动 (Direct I/O, LCG, 设备管理)
│   └── gui/                       # 界面与绘图组件 (egui)
└── README.md
```
