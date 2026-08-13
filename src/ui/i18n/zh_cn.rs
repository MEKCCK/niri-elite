use super::Messages;

pub static MESSAGES: Messages = Messages {
    locale: "zh-CN",
    cancel: "取消",
    share: "共享",
    share_screen: "共享屏幕",
    remember_selection: "记住此选择",
    window: "窗口",
    display: "显示器",
    untitled_window: "无标题窗口",
    protected_window: "受保护窗口",
    hidden_from_screen_share: "已从屏幕共享中隐藏",
    share_description,
};

fn share_description(app_id: Option<&str>) -> String {
    match app_id {
        Some(app_id) => format!("{app_id} 想要共享你的屏幕。请选择要共享的内容。"),
        None => String::from("有应用想要共享你的屏幕。请选择要共享的内容。"),
    }
}
