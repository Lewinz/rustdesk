# RustDesk 定制版（键盘映射增强）

基于 [RustDesk](https://github.com/rustdesk/rustdesk) 的自用 fork，核心增强：**macOS 控制端全屏远程时，将本机键盘事件（含系统级快捷键）全部拦截并转发到被控端**，达到类似 VMware/Parallels 全屏控制虚拟机的体验。

## 功能

### macOS 全屏键盘抓取（核心）

控制端为 macOS、远程窗口全屏且聚焦时：

- **所有键盘事件被拦截**并转发到被控端，包括系统级快捷键：
  - `Cmd+Space`（输入法 / Spotlight）→ 触发被控端而非本机
  - `Mission Control`（Ctrl+↑）等系统快捷键 → 直达被控端
  - `双击 Cmd`（Raycast 唤醒等修饰键类全局快捷键）→ 不再触发本机
- **鼠标移到悬浮工具栏上时键盘抓取保持**（不再因指针离开画面而断开）
- 退出全屏 / 窗口失焦 / 断连 → 自动释放，恢复默认行为

### 白名单放行（设置 → 操控）

全屏抓取激活时，可配置**放行组合键**：这些组合保留在本机生效，不转发被控端。

- 列表式管理：名称 / 快捷键 / 删除 三列，可增删行、改名称
- **录制式录入**：点击快捷键按钮 → 全局接管键盘（录制期间任何组合键都不会触发本机）→ 直接按组合键录入（如 `Cmd+Shift+T`，显示为 `⇧⌘T`）；Esc 取消
- 支持：`cmd/ctrl/alt/option/shift` + 字母 / F1-F12 / Tab / Space / 方向键 / Delete / Enter / Home / End / PageUp / PageDown / 数字键码
- 修改即时生效，无需重连

### 其他

- 触控板**双指滚动（含左右横向）**转发被控端（RustDesk 原有输入通道，全屏抓取时保持激活）
- 全屏键盘抓取开关（设置 → 操控 → 开启全屏拦截键盘事件）

## 架构

- 控制端 Rust 层新增 `CGEventTap`（`kCGHIDEventTap`，见 `src/platform/macos.mm` 与 `src/platform/macos_keyboard_capture.rs`）：拦截 keyDown/keyUp/flagsChanged，返回 NULL 吞掉本地事件
- 拦截的事件喂给**现有键盘编码路径**（`keyboard::client::process_event_with_session`）转发被控端，被控端零改动
- 录制模式使用独立的临时 tap，通过 Rust→Dart 事件流（`GLOBAL_EVENT_STREAM`）回传组合键
- 设置项：`macos-fullscreen-keyboard-capture`（开关）、`macos-fullscreen-keyboard-capture-whitelist`（白名单 JSON）

## 构建

```bash
# 环境要求：Flutter 3.24.x、Xcode（部署目标需 ≥12.0）、CocoaPods
./build_self_macos.sh
```

脚本自动完成：cargo release 编译 →（必要时）重新生成 flutter_rust_bridge 绑定 → flutter build macos → 统一 ad-hoc 重签。产物：`flutter/build/macos/Build/Products/Release/RustDeskSelf.app`

> 每次重新构建后 app 的 ad-hoc 签名会变化，macOS 的**辅助功能授权**可能失效——首次使用需在 系统设置 → 隐私与安全性 → 辅助功能 重新勾选。拷贝到其他机器后先执行 `xattr -dr com.apple.quarantine <app>`。

## 已知限制（macOS 系统约束）

- **`Cmd+Tab`（App 切换器）无法被用户态程序拦截**（WindowServer 层处理），按 Cmd+Tab 仍会切换本机应用；切换后窗口失焦自动释放抓取
- **触控板系统手势**（三指滑动切换 Space、四指 Mission Control、五指 Launchpad）无法拦截，这正是"三指滑动逃出键盘抓取"的设计依据
- **手势事件（swipe/magnify）无法跨应用注入**，浏览器"双指轻扫翻页"无法原样转发到被控端；慢速横向滚动可转发

## 打包隔离（可选）

仓库内的 macOS 工程已配置为独立应用标识（`com.carriez.rustdesk-self` / RustDeskSelf），可与官方版同时运行、互不影响。如需进一步隔离配置目录（ID/设置），可修改 `libs/hbb_common/src/config.rs` 中 `APP_NAME` 默认值（注意：该文件属于 hbb_common submodule，改动需自行维护）。
