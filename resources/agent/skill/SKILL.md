---
name: niri-agent
description: Operate GUI apps on the user's niri desktop in the background through niri's agent seat (`niri msg agent`) — capture a window, click, type, press keys, launch apps — without moving the user's cursor or focus. Use when asked to operate, click, fill in, read or check something in a desktop app, browser or other GUI window on this machine. 操作桌面应用、点按钮、填表、在浏览器里打开网页、看某个窗口的画面时使用。
---

# 在 niri 桌面上操作图形界面应用

你有自己的一套指针和键盘（agent seat），输入只进你指定 id 的窗口，不影响用户的光标、焦点和剪贴板。窗口在不在屏幕上都能操作。下文的 `agent-ui`、`agent-firefox` 在本文件同目录 `~/.claude/skills/niri-agent/`。

## 选窗口

`niri msg -j windows`：`is_focused: true` 是用户正在用的，除非用户要求，不要碰；`is_agent: true` 是你自己开的。

**共享模式**：直接操作用户开着的窗口。GTK、Qt、Firefox、Alacritty 可以；Chromium、Electron（VS Code、QQ）、kitty、Zed 只认一个 seat，会报 `only handles one seat`，改用独立模式。

**独立模式**：`niri msg agent launch -- 命令`，等到第一个窗口出现，打印含 `id` 的 JSON。任何应用都能操作，开在 agent 工作区，不抢焦点。环境变量继承自 niri，不是你的 shell；要设就写 `launch -- env FOO=bar 命令`。

单实例应用必须用独立 profile，否则请求交给用户的进程，30 秒后报错：

| 应用 | 启动方式 |
|---|---|
| Firefox（上网首选） | `agent-firefox URL`：先复制用户当前的登录状态，再启动；已开着就开新窗口 |
| Chromium | `launch -- env ACCESSIBILITY_ENABLED=1 chromium --user-data-dir="$HOME/.local/share/niri-agent/chromium" --force-renderer-accessibility --no-first-run URL` |
| VS Code | `launch -- code --user-data-dir="$HOME/.local/share/niri-agent/vscode"` |
| Mousepad | `launch -- mousepad --disable-server` |

agent-firefox 里是用户的真实账号：不要向用户要密码，也不要替用户发帖、发消息、下单、改设置。

## 循环：定位 → 动作 → 看回执

**定位，优先用 `agent-ui`**：读应用的无障碍树，几百 token，名字是原文。

```sh
agent-ui --id 7                 # 全部可见控件
agent-ui --id 7 --find 首选项   # 名字、文字里含"首选项"的
```
```
menu item '首选项...' @ 227,521.5 (324x31)
check box '使用系统等宽字体' @ 119.5,368.5 (197x27) [checked]
spin button @ 299,216 (128x34) [disabled; text='80'; value=80]
```

`@ X,Y` 是窗口坐标，直接 `click --id 7 X Y`，不加 `--image`。kitty、Alacritty、游戏没有无障碍树，会报错；要看颜色、布局、图片内容时也要截图。

**截图**，再用 Read 看：

```sh
niri msg agent capture --id 7 --path /tmp/niri-agent/7.png --max-size 1280
```

token 只取决于像素尺寸，1280 一般够用。截图包含阴影和展开的菜单。之后按图片像素坐标操作时加 `--image 那张图`，换了窗口就重截。

**动作**：

```sh
niri msg agent click  --id 7 --image /tmp/niri-agent/7.png 288 29   # --button right / --count 2
niri msg agent move   --id 7 --image /tmp/niri-agent/7.png 288 29   # 悬停
niri msg agent scroll --id 7 --image /tmp/niri-agent/7.png 300 500 --dy 400   # 正数向下，逻辑像素
niri msg agent key    --id 7 ctrl+shift+t                           # xkb 键名：Return Escape Tab BackSpace Page_Down F5…
printf '%s' '任意 Unicode 文字' | niri msg agent type --id 7 -      # 一律从标准输入传，不经过输入法
```

拖动：`button --pressed true`，`move`，`button --pressed false`。

多步用 `--require-change` 加 `&&` 串成一条命令。某步没有引起任何变化时，以退出码 3 停下，后面的不执行。悬停这类不一定有变化的步骤不要加：

```sh
niri msg agent --require-change click --id 7 304 120 &&
printf '%s' '张三' | niri msg agent type --id 7 - &&
niri msg agent --require-change key --id 7 ctrl+s
```

**回执**：每个动作等画面稳定（最多 2 秒）后打印 JSON，坐标是窗口坐标。

| 回执 | 含义 → 做法 |
|---|---|
| `changed`、`popups_opened`、`windows_opened` 全空 | 没点中 → 重新定位，不要原地再点 |
| `popups_opened` 非空，`focus: "popup"` | 菜单打开了 → `agent-ui --find` 找菜单项；按键会进菜单 |
| `windows_opened` | 新窗口或对话框，`parent_id` 是父窗口 → 改用新 `id` |
| `title` | 标题变了：页面加载完成，或文件未保存 |
| `settled: false` | 还在变（加载、动画、视频）→ 稍等再看 |
| 文本框里约 8 像素宽的 `changed` | 光标闪烁 → 忽略 |

回执能说明结果，就不要截图；需要知道变成什么样时，先 `agent-ui`，不够再截图。

## 注意

- 不关用户的窗口；不可撤销的操作（发送、付款、删除）要用户明确要求才做。
- 你的剪贴板是独立的，`ctrl+c` 的内容谁都读不到。要拿内容，就截图，或让应用存成文件。
- 锁屏期间照常工作，不要碰锁屏界面。

| 报错 | 处理 |
|---|---|
| `only handles one seat` | 改用独立模式 |
| `window N was not found` | 重新 `niri msg -j windows` |
| `no window appeared within 30 s` | 按上表加 profile 参数 |
| `is a capture of window A, not window B` | 给目标窗口重新截图 |
| `the agent seat is off` | 告诉用户，不要改配置 |
