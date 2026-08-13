use super::Messages;

pub static MESSAGES: Messages = Messages {
    locale: "en-US",
    cancel: "Cancel",
    share: "Share",
    share_screen: "Share Screen",
    remember_selection: "Remember this selection",
    window: "Window",
    display: "Display",
    untitled_window: "Untitled window",
    protected_window: "Protected window",
    hidden_from_screen_share: "Hidden from screen sharing",
    share_description,
};

fn share_description(app_id: Option<&str>) -> String {
    match app_id {
        Some(app_id) => {
            format!("{app_id} wants to share your screen. Choose what you would like to share.")
        }
        None => {
            String::from("An app wants to share your screen. Choose what you would like to share.")
        }
    }
}
