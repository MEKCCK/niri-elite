---
name: niri-agent
description: Operate GUI apps on the user's niri desktop in the background through niri's agent seat (`niri msg agent`) — capture a window, click, type, press keys, launch apps — without moving the user's cursor or focus. Use when asked to operate, click, fill in, read or check something in a desktop app, browser or other GUI window on this machine. 操作桌面应用、点按钮、填表、在浏览器里打开网页、看某个窗口的画面时使用。
---

# 在 niri 桌面上操作图形界面应用

niri 给你单独开了一个 seat（一套独立的指针和键盘）。你的点击和输入只进你指定的窗口，用户的光标、焦点和剪贴板都不受影响，用户可以同时正常工作。所有操作都按**窗口 id** 进行，窗口在不在屏幕上、用户正在看哪里都没关系。

先确认可用：`niri msg agent --help` 能列出子命令就行。如果命令报 `the agent seat is off`，告诉用户 niri 配置里关掉了这个功能，不要自己去改配置。

## 选择要操作的窗口

```sh
niri msg -j windows
```

每个窗口有 `id`、`app_id`、`title`、`is_focused`、`is_agent`。

- `is_focused: true` 是用户此刻正在用的窗口。除非用户明确让你操作它，否则不要碰：你的输入会和用户的输入交错。
- `is_agent: true` 是你自己用 `launch` 开的窗口。

## 两种方式

**直接操作用户已经开着的窗口（共享模式）。** GTK 应用、Firefox、Alacritty 这类终端都可以。Chromium 和 Electron 应用（VS Code、QQ……）、kitty、Zed 不行：它们只认一个 seat，命令会报错 `doesn't listen to the agent pointer: its app only handles one seat`。遇到这个报错，改用下面的 `launch`。

**自己启动一个应用（独立模式）。**

```sh
niri msg agent launch -- chromium --user-data-dir="$HOME/.local/share/niri-agent/chromium" --no-first-run https://example.com
```

它会等到应用的第一个窗口出现，打印一行 JSON，里面有窗口 `id`。这样启动的应用：

- 任何应用都能操作，包括 Chromium、Electron 和 kitty。
- 开在单独的工作区里，不抢用户的焦点，用户也没法在里面打字。
- 是受限的：不能截屏、不能读写用户的剪贴板，也没有 `NIRI_SOCKET`。
- 继承的是 niri 的环境变量，不是你这个 shell 的。要设环境变量，写成 `launch -- env FOO=bar 应用`。

**上网用 Firefox，而且一律通过这个脚本启动**（它和本文件在同一个目录）：

```sh
~/.claude/skills/niri-agent/agent-firefox https://example.com
```

它在启动前把用户 Firefox 里当前的登录状态（cookies、网站存储、保存的密码）复制到你专用的 profile，然后用 `launch` 启动，打印同样的 JSON。用户登录过的网站，你这边也是登录状态，不要向用户要密码。你的 Firefox 已经开着时，它在里面开一个新窗口，这时不复制。只复制一个方向：你在里面登录、退出，都影响不到用户的 Firefox，下次启动还会被覆盖。

这些登录是用户的账号：只做用户要你做的事，不要替用户发帖、发消息、下单、改账号设置。

其他浏览器和"单实例"应用一定要用单独的 profile 或实例参数，否则请求会转交给用户已经开着的那个进程，`launch` 等 30 秒后报错：

| 应用 | 参数 |
|---|---|
| Chromium / Chrome | `--user-data-dir="$HOME/.local/share/niri-agent/chromium"` |
| VS Code | `--user-data-dir="$HOME/.local/share/niri-agent/vscode"` |
| Mousepad | `--disable-server` |

这些 profile 里没有用户的登录状态。

## 标准循环：截图 → 看图 → 动作 → 看回执

**1. 截图**，存成文件后用 Read 工具看：

```sh
mkdir -p /tmp/niri-agent
niri msg agent capture --id 7 --path /tmp/niri-agent/7.png --max-size 1280
```

- `--max-size 1280` 让长边不超过 1280 像素，太小会看不清，太大浪费 token。
- 图片包括窗口的阴影和展开的菜单。截图前会先等窗口交出最新的一帧。
- 旁边会写一份 `7.png.json`，记录图片和窗口坐标的对应关系，下一步要用。

**2. 动作**：直接用图片上的像素坐标，加 `--image` 指向那张截图，CLI 会自动换算。

```sh
niri msg agent click  --id 7 --image /tmp/niri-agent/7.png 288 29
niri msg agent click  --id 7 --image /tmp/niri-agent/7.png 288 29 --button right
niri msg agent click  --id 7 --image /tmp/niri-agent/7.png 288 29 --count 2      # 双击
niri msg agent move   --id 7 --image /tmp/niri-agent/7.png 288 29                # 悬停，比如看提示框
niri msg agent scroll --id 7 --image /tmp/niri-agent/7.png 300 500 --dy 400      # 正数向下，单位是逻辑像素
niri msg agent key    --id 7 ctrl+l
printf '%s' 'https://example.com' | niri msg agent type --id 7 -
niri msg agent key    --id 7 Return
```

- **打字一律用 `type --id N -`，从标准输入传文字**，避免引号、`$`、换行被 shell 处理。任何 Unicode 都行，中文直接打，不经过输入法。
- 按键名是 xkb 的键名：`Return`、`Escape`、`Tab`、`BackSpace`、`Delete`、`Up`、`Down`、`Left`、`Right`、`Page_Down`、`Home`、`F5`……修饰键写 `ctrl`、`shift`、`alt`、`super`，用 `+` 连起来，比如 `ctrl+shift+t`。
- 拖动：`button --pressed true` 按下，`move` 到目标，`button --pressed false` 松开，都带同一个 `--image`。
- 窗口 id 必须和截图对应。截图换了窗口，就重新截。

**3. 看回执。** 每个动作都会等窗口画面稳定（最多 2 秒），然后打印一行 JSON：

```json
{"settled":true,"elapsed_ms":62,"changed":[{"x":26,"y":62,"w":58,"h":24}],"resized":null,
 "popups_opened":[{"x":26,"y":86,"w":240,"h":380}],"popups_closed":[],
 "windows_opened":[],"windows_closed":[],"title":null,"focus":"popup"}
```

坐标都是窗口坐标，不是图片像素。怎么读：

| 回执 | 说明 | 下一步 |
|---|---|---|
| `changed`、`popups_opened`、`windows_opened` 全是空的 | 什么都没发生，多半点偏了，或者点的不是控件 | 重新截图定位，不要假设已经成功 |
| `popups_opened` 不为空，`focus` 是 `"popup"` | 菜单或下拉框打开了 | 重新截图，选菜单项；按键会进菜单 |
| `windows_opened` 有新窗口 | 开了新窗口或对话框；`parent_id` 是它的父窗口 | 之后操作新窗口的 `id` |
| `title` 不是 null | 窗口标题变了，比如页面加载完成、文件变成"未保存" | |
| `settled: false` | 2 秒内画面一直在变：页面在加载、视频在放、动画没停 | 稍等一会儿再截图 |
| `windows_closed` 里有目标窗口 | 目标窗口关掉了 | |
| `changed` 里有一条约 8 像素宽的窄条，位置在文本框里 | 文本光标在闪，不算真正的变化 | 忽略 |
| 点了菜单项，但 `popups_closed` 为空、其他也都没变 | 共享模式下 GTK 3 应用的菜单项偶尔不认点击 | 改用键盘：方向键选到这一项再按 `Return`。菜单本身也可以用 `alt+助记字母` 打开，比如"编辑(E)"就是 `alt+e` |

只有回执说明画面变了、而且你需要看清变成了什么时，才重新截图。

## 注意

- 不要碰用户正在用的窗口，不要关闭用户的窗口，不要替用户做不可撤销的操作（发送消息、付款、删除），除非用户明确要求。
- 你的剪贴板和用户的是分开的。你在应用里按 `ctrl+c` 复制的东西，用户粘贴不到，你自己也读不到。要把内容带出来，就截图看，或者让应用把内容存成文件。
- 截图里显示的就是用户屏幕上会看到的内容。用户在 niri 配置里设了屏蔽录屏的窗口，在截图里是黑块。
- 锁屏期间可以照常工作。锁屏界面本身你碰不到，也不要尝试解锁。

## 常见报错

| 报错 | 处理 |
|---|---|
| `doesn't listen to the agent pointer/keyboard: its app only handles one seat` | 这个应用只认一个 seat。用 `launch` 另开一个实例 |
| `window N was not found` | 窗口关了或 id 变了，重新 `niri msg -j windows` |
| `no window appeared within 30 s` | 应用把请求交给了已经开着的实例，加上上表里的 profile 参数；Firefox 用 `agent-firefox` |
| `"x.png" is a capture of window A, not window B` | 截图和窗口对不上，给这个窗口重新截图 |
| `the agent seat is off` | 用户关掉了这个功能，告诉用户 |
