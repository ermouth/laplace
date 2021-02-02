use std::{convert::TryFrom, result::Result as StdResult};

use anyhow::{anyhow, Context, Error, Result};
use dapla_common::{
    api::{Response as CommonDapResponse, UpdateQuery},
    dap::{Dap as CommonDap, Permission},
};
use yew::{
    html, initialize, run_loop,
    services::{fetch::Response, ConsoleService},
    utils, App, Callback, Component, ComponentLink, Html,
};
use yew_mdc_widgets::{
    auto_init,
    utils::dom::{select_exist_element, JsObjectAccess, JsValue},
    Chip, ChipSet, CustomEvent, Drawer, Element, IconButton, MdcWidget, Switch, TopAppBar,
};

use self::fetch::Fetcher;

mod fetch;

type Dap = CommonDap<String>;
type DapResponse = CommonDapResponse<'static, String>;
type StringResponse = Response<Result<String>>;

trait RootMsgError {
    type Map;

    fn msg_error(self, link: &ComponentLink<Root>);
    fn msg_error_map(self, link: &ComponentLink<Root>) -> Self::Map;
}

impl<T> RootMsgError for Result<T> {
    type Map = StdResult<T, ()>;

    fn msg_error(self, link: &ComponentLink<Root>) {
        if let Err(err) = self {
            link.send_message(Msg::Error(err))
        }
    }

    fn msg_error_map(self, link: &ComponentLink<Root>) -> Self::Map {
        self.map_err(|err| link.send_message(Msg::Error(err)))
    }
}

struct Root {
    daps: Vec<Dap>,
    link: ComponentLink<Self>,
    fetcher: Fetcher,
}

#[derive(Debug)]
struct PermissionUpdate {
    dap_name: String,
    permission: Permission,
    allow: bool,
}

impl PermissionUpdate {
    fn try_from_chip_selection_detail(detail: JsValue) -> Result<Self> {
        let chip_id = detail
            .get("chipId")
            .as_string()
            .ok_or_else(|| anyhow!("Detail chipId param does not exist"))?;
        let id_data: Vec<_> = chip_id.split("--").collect();
        if let (Some(dap_name), Some(permission)) = (id_data.get(0), id_data.get(1)) {
            Ok(Self {
                dap_name: dap_name.to_string(),
                permission: Permission::try_from(*permission)?,
                allow: detail
                    .get("selected")
                    .as_bool()
                    .ok_or_else(|| anyhow!("Detail selected param does not exist"))?,
            })
        } else {
            Err(anyhow!("Wrong data of chipId: {:?}", id_data))
        }
    }
}

#[derive(Debug)]
enum Msg {
    Fetch(DapResponse),
    SwitchDap(String),
    UpdatePermission(PermissionUpdate),
    Error(Error),
}

impl Component for Root {
    type Message = Msg;
    type Properties = ();

    fn create(_props: Self::Properties, link: ComponentLink<Self>) -> Self {
        let mut root = Self {
            daps: vec![],
            link,
            fetcher: Fetcher::new(),
        };

        root.send_get(Dap::main_uri("daps"))
            .context("Get daps list error")
            .msg_error(&root.link);
        root
    }

    fn update(&mut self, msg: Self::Message) -> bool {
        match msg {
            Msg::Fetch(response) => match response {
                DapResponse::Daps(daps) => {
                    self.daps = daps.into_iter().map(|dap| dap.into_owned()).collect();
                    true
                }
                DapResponse::Updated(updated) => {
                    if let Some(dap) = self.daps.iter_mut().find(|dap| dap.name() == updated.dap_name) {
                        let mut should_render = false;

                        if let Some(enabled) = updated.enabled {
                            if dap.enabled() != enabled {
                                dap.set_enabled(enabled);
                                should_render = true;
                            }
                        }

                        if let Some(permission) = updated.allow_permission {
                            should_render = dap.allow_permission(permission);
                        }

                        if let Some(permission) = updated.deny_permission {
                            should_render = dap.deny_permission(permission);
                        }

                        should_render
                    } else {
                        ConsoleService::error(&format!("Unknown dap name: {}", updated.dap_name));
                        false
                    }
                }
            },
            Msg::SwitchDap(name) => {
                if let Some(dap) = self.daps.iter_mut().find(|dap| dap.name() == name) {
                    dap.switch_enabled();

                    let uri = Dap::main_uri("dap");
                    if let Ok(body) = serde_json::to_string(
                        &UpdateQuery::new(dap.name().to_string())
                            .enabled(dap.enabled())
                            .into_request(),
                    )
                    .context("Serialize query error")
                    .msg_error_map(&self.link)
                    {
                        self.send_post(uri, body)
                            .context("Switch dap error")
                            .msg_error(&self.link);
                    }
                    false
                } else {
                    ConsoleService::error(&format!("Unknown dap name: {}", name));
                    false
                }
            }
            Msg::UpdatePermission(PermissionUpdate {
                dap_name,
                permission,
                allow,
            }) => {
                let uri = Dap::main_uri("dap");
                if let Ok(body) = serde_json::to_string(
                    &UpdateQuery::new(dap_name)
                        .update_permission(permission, allow)
                        .into_request(),
                )
                .context("Serialize query error")
                .msg_error_map(&self.link)
                {
                    self.send_post(uri, body)
                        .context("Select permission error")
                        .msg_error(&self.link);
                }
                false
            }
            Msg::Error(err) => {
                ConsoleService::error(&format!("{}", err));
                true
            }
        }
    }

    fn change(&mut self, _props: Self::Properties) -> bool {
        false
    }

    fn view(&self) -> Html {
        let drawer = Drawer::new()
            .id("app-drawer")
            .title(html! { <h3 tabindex = 0>{ "Settings" }</h3> })
            .modal();

        let top_app_bar = TopAppBar::new()
            .id("top-app-bar")
            .title("dapla")
            .navigation_item(IconButton::new().icon("menu"))
            .enable_shadow_when_scroll_window()
            .on_navigation(|_| {
                let drawer = select_exist_element::<Element>("#app-drawer").get("MDCDrawer");
                let opened = drawer.get("open").as_bool().unwrap_or(false);
                drawer.set("open", !opened);
            });

        html! {
            <>
                { drawer }
                <div class="mdc-drawer-scrim"></div>

                <div class = vec!["app-content", Drawer::APP_CONTENT_CLASS]>
                    { top_app_bar }

                    <div class = "mdc-top-app-bar--fixed-adjust">
                        <div class = "content-container">
                            <h1 class = "title mdc-typography--headline5">{ "Applications" }</h1>
                            <div class = "daps-table">
                                { self.daps.iter().map(|dap| self.view_dap(dap)).collect::<Html>() }
                            </div>
                        </div>
                    </div>
                </div>
            </>
        }
    }

    fn rendered(&mut self, _first_render: bool) {
        auto_init();
    }
}

impl Root {
    pub fn send_get(&mut self, uri: impl AsRef<str>) -> Result<()> {
        let callback = self.fetch_callback(Msg::Fetch);
        self.fetcher.send_get(uri, callback)
    }

    pub fn send_post(&mut self, uri: impl AsRef<str>, body: impl Into<String>) -> Result<()> {
        let callback = self.fetch_callback(Msg::Fetch);
        self.fetcher.send_post(uri, body, callback)
    }

    fn fetch_callback(&self, callback: impl Fn(DapResponse) -> Msg + 'static) -> Callback<StringResponse> {
        self.link.callback(move |response: StringResponse| {
            if response.status().is_success() {
                match Fetcher::parse(response).context("Can't parse response") {
                    Ok(response) => callback(response),
                    Err(err) => Msg::Error(err),
                }
            } else {
                Msg::Error(anyhow!(
                    "Fetch status: {:?}, body: {:?}",
                    response.status(),
                    response.into_body()
                ))
            }
        })
    }

    fn view_dap(&self, dap: &Dap) -> Html {
        let dap_name = dap.name().to_string();

        let enable_switch = Switch::new()
            .on_click(self.link.callback(move |_| Msg::SwitchDap(dap_name.clone())))
            .turn(dap.enabled());

        let permissions = ChipSet::new()
            .id(format!("{}--permissions", dap.name()))
            .filter()
            .chips(dap.required_permissions().map(|permission| {
                Chip::simple()
                    .id(format!("{}--{}", dap.name(), permission.as_str()))
                    .checkmark()
                    .text(permission.as_str())
                    .select(dap.is_allowed_permission(permission))
            }))
            .on_selection(self.link.callback(|event: CustomEvent| {
                PermissionUpdate::try_from_chip_selection_detail(event.detail())
                    .map(Msg::UpdatePermission)
                    .unwrap_or_else(Msg::Error)
            }));

        html! {
            <>
                <div class = "daps-table-row">
                    <div class = "daps-table-col">
                        <big><a href = dap.name()>{ dap.title() }</a></big>
                    </div>
                    <div class = "daps-table-col">
                        { enable_switch }
                    </div>
                </div>
                <div class = "daps-table-row">
                    <div class = "daps-table-col">
                        { permissions }
                    </div>
                </div>
            </>
        }
    }
}

fn main() {
    initialize();
    if let Ok(Some(root)) = utils::document().query_selector("#root") {
        App::<Root>::new().mount_with_props(root, ());
        run_loop();
    } else {
        ConsoleService::error("Can't get root node for rendering");
    }
}
