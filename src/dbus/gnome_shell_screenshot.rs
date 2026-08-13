use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::path::PathBuf;

use niri_ipc::PickedColor;
use zbus::fdo::{self, RequestNameFlags};
use zbus::zvariant::OwnedValue;
use zbus::{interface, zvariant};

use super::Start;
use crate::ui::screenshot_ui::{
    ScreenshotPathReplySender, ScreenshotPortalError, ScreenshotSelectionReplySender,
};

pub struct Screenshot {
    to_niri: calloop::channel::Sender<ScreenshotToNiri>,
}

pub enum ScreenshotToNiri {
    TakeScreenshot {
        include_cursor: bool,
        reply: ScreenshotPathReplySender,
    },
    InteractiveScreenshot(ScreenshotPathReplySender),
    TakeWindow {
        include_cursor: bool,
        reply: ScreenshotPathReplySender,
    },
    SelectArea(ScreenshotSelectionReplySender),
    TakeArea {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        reply: ScreenshotPathReplySender,
    },
    CancelScreenshot,
    PickColor(async_channel::Sender<Option<PickedColor>>),
}

fn portal_error(error: ScreenshotPortalError) -> fdo::Error {
    match error {
        ScreenshotPortalError::Cancelled => {
            fdo::Error::Failed("screenshot was canceled".to_owned())
        }
        ScreenshotPortalError::Failed(message) => fdo::Error::Failed(message),
    }
}

async fn receive_path(
    receiver: async_channel::Receiver<Result<PathBuf, ScreenshotPortalError>>,
) -> fdo::Result<PathBuf> {
    receiver
        .recv()
        .await
        .map_err(|err| fdo::Error::Failed(format!("screenshot reply channel closed: {err}")))?
        .map_err(portal_error)
}

pub(super) fn file_uri(path: &Path) -> fdo::Result<String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|err| fdo::Error::Failed(format!("cannot resolve screenshot path: {err}")))?
            .join(path)
    };

    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(char::from(*byte));
            }
            _ => {
                use std::fmt::Write as _;
                write!(&mut uri, "%{byte:02X}").unwrap();
            }
        }
    }

    Ok(uri)
}

#[interface(name = "org.gnome.Shell.Screenshot")]
impl Screenshot {
    async fn screenshot(
        &self,
        include_cursor: bool,
        _flash: bool,
        _filename: PathBuf,
    ) -> fdo::Result<(bool, PathBuf)> {
        let (reply, receiver) = async_channel::bounded(1);
        if let Err(err) = self.to_niri.send(ScreenshotToNiri::TakeScreenshot {
            include_cursor,
            reply,
        }) {
            warn!("error sending message to niri: {err:?}");
            return Err(fdo::Error::Failed("internal error".to_owned()));
        }

        let filename = receive_path(receiver).await?;
        Ok((true, filename))
    }

    async fn interactive_screenshot(&self) -> fdo::Result<(bool, String)> {
        let (reply, receiver) = async_channel::bounded(1);
        self.to_niri
            .send(ScreenshotToNiri::InteractiveScreenshot(reply))
            .map_err(|err| fdo::Error::Failed(format!("error sending message to niri: {err}")))?;

        match receiver.recv().await {
            Ok(Ok(path)) => Ok((true, file_uri(&path)?)),
            Ok(Err(ScreenshotPortalError::Cancelled)) => Ok((false, String::new())),
            Ok(Err(error)) => Err(portal_error(error)),
            Err(err) => Err(fdo::Error::Failed(format!(
                "screenshot reply channel closed: {err}"
            ))),
        }
    }

    async fn screenshot_window(
        &self,
        _include_frame: bool,
        include_cursor: bool,
        _flash: bool,
        _filename: PathBuf,
    ) -> fdo::Result<(bool, PathBuf)> {
        let (reply, receiver) = async_channel::bounded(1);
        self.to_niri
            .send(ScreenshotToNiri::TakeWindow {
                include_cursor,
                reply,
            })
            .map_err(|err| fdo::Error::Failed(format!("error sending message to niri: {err}")))?;

        Ok((true, receive_path(receiver).await?))
    }

    async fn select_area(&self) -> fdo::Result<(i32, i32, i32, i32)> {
        let (reply, receiver) = async_channel::bounded(1);
        self.to_niri
            .send(ScreenshotToNiri::SelectArea(reply))
            .map_err(|err| fdo::Error::Failed(format!("error sending message to niri: {err}")))?;

        receiver
            .recv()
            .await
            .map_err(|err| fdo::Error::Failed(format!("selection reply channel closed: {err}")))?
            .map_err(portal_error)
    }

    async fn screenshot_area(
        &self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        _flash: bool,
        _filename: PathBuf,
    ) -> fdo::Result<(bool, PathBuf)> {
        let (reply, receiver) = async_channel::bounded(1);
        self.to_niri
            .send(ScreenshotToNiri::TakeArea {
                x,
                y,
                width,
                height,
                reply,
            })
            .map_err(|err| fdo::Error::Failed(format!("error sending message to niri: {err}")))?;

        Ok((true, receive_path(receiver).await?))
    }

    async fn cancel_screenshot(&self) -> fdo::Result<()> {
        self.to_niri
            .send(ScreenshotToNiri::CancelScreenshot)
            .map_err(|err| {
                fdo::Error::Failed(format!("error sending cancellation to niri: {err}"))
            })?;
        Ok(())
    }

    async fn pick_color(&self) -> fdo::Result<HashMap<String, OwnedValue>> {
        let (tx, rx) = async_channel::bounded(1);
        if let Err(err) = self.to_niri.send(ScreenshotToNiri::PickColor(tx)) {
            warn!("error sending pick color message to niri: {err:?}");
            return Err(fdo::Error::Failed("internal error".to_owned()));
        }

        let color = match rx.recv().await {
            Ok(Some(color)) => color,
            Ok(None) => {
                return Err(fdo::Error::Failed("no color picked".to_owned()));
            }
            Err(err) => {
                warn!("error receiving message from niri: {err:?}");
                return Err(fdo::Error::Failed("internal error".to_owned()));
            }
        };

        let mut result = HashMap::new();
        let [r, g, b] = color.rgb;
        result.insert(
            "color".to_string(),
            zvariant::OwnedValue::try_from(zvariant::Value::from((r, g, b))).unwrap(),
        );

        Ok(result)
    }
}

impl Screenshot {
    pub fn new(to_niri: calloop::channel::Sender<ScreenshotToNiri>) -> Self {
        Self { to_niri }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::file_uri;

    #[test]
    fn file_uri_percent_encodes_path_bytes() {
        assert_eq!(
            file_uri(Path::new("/tmp/a screenshot #1.png")).unwrap(),
            "file:///tmp/a%20screenshot%20%231.png"
        );
    }
}

impl Start for Screenshot {
    fn start(self) -> anyhow::Result<zbus::blocking::Connection> {
        let conn = zbus::blocking::Connection::session()?;
        let flags = RequestNameFlags::AllowReplacement
            | RequestNameFlags::ReplaceExisting
            | RequestNameFlags::DoNotQueue;

        conn.object_server()
            .at("/org/gnome/Shell/Screenshot", self)?;
        conn.request_name_with_flags("org.gnome.Shell.Screenshot", flags)?;

        Ok(conn)
    }
}
