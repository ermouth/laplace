use std::ops::Deref;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use borsh::BorshDeserialize;
use derive_more::{Deref, DerefMut};
pub use laplace_common::api::{UpdateQuery, UpdateRequest as LappUpdateRequest};
pub use laplace_common::lapp::access::*;
use laplace_wasm::http::{Request, Response};
use reqwest::blocking::Client;
use rusqlite::Connection;
use serde::{Serialize, Serializer};
use tokio::fs;
use tokio::sync::{RwLock, RwLockReadGuard};
use wasmer::{Exports, Function, FunctionEnv, Imports, Instance, Module, Store};
use wasmer_wasi::WasiState;

use crate::error::{ServerError, ServerResult};
use crate::lapps::settings::{FileSettings, LappSettings, LappSettingsResult};
use crate::lapps::wasm_interop::database::{self, DatabaseEnv};
use crate::lapps::wasm_interop::http::{self, HttpEnv};
use crate::lapps::wasm_interop::{sleep, MemoryManagementHostData};
use crate::lapps::{LappInstance, LappInstanceError};
use crate::service;

pub type CommonLapp = laplace_common::lapp::Lapp<PathBuf>;
pub type CommonLappResponse<'a> = laplace_common::api::Response<'a, PathBuf, CommonLappGuard<'a>>;

pub struct CommonLappGuard<'a>(pub RwLockReadGuard<'a, Lapp>);

impl Deref for CommonLappGuard<'_> {
    type Target = CommonLapp;

    fn deref(&self) -> &Self::Target {
        &*self.0
    }
}

impl Serialize for CommonLappGuard<'_> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.lapp.serialize(serializer)
    }
}

#[derive(Deref, DerefMut)]
pub struct Lapp {
    #[deref]
    #[deref_mut]
    lapp: CommonLapp,
    instance: Option<LappInstance>,
    service_sender: Option<service::lapp::Sender>,
}

impl Lapp {
    pub fn new(name: impl Into<String>, root_dir: impl Into<PathBuf>) -> Self {
        let mut lapp = Self {
            lapp: CommonLapp::new(name.into(), root_dir.into(), Default::default()),
            instance: None,
            service_sender: None,
        };
        if !lapp.is_main() {
            if let Err(err) = lapp.reload_settings() {
                log::error!("Error when load settings for lapp '{}': {err:?}", lapp.name());
            }
        }
        lapp
    }

    pub const fn settings_file_name() -> &'static str {
        "settings.toml"
    }

    pub const fn static_dir_name() -> &'static str {
        CommonLapp::static_dir_name()
    }

    pub const fn index_file_name() -> &'static str {
        CommonLapp::index_file_name()
    }

    pub const fn main_name() -> &'static str {
        CommonLapp::main_name()
    }

    pub fn main_static_uri() -> String {
        CommonLapp::main_static_uri()
    }

    pub fn main_uri(tail: impl AsRef<str>) -> String {
        CommonLapp::main_uri(tail)
    }

    pub fn reload_settings(&mut self) -> LappSettingsResult<()> {
        self.lapp
            .set_settings(LappSettings::load(self.root_dir().join(Self::settings_file_name()))?);
        Ok(())
    }

    pub fn save_settings(&mut self) -> LappSettingsResult<()> {
        let path = self.root_dir().join(Self::settings_file_name());
        self.settings().save(path)
    }

    pub fn static_dir(&self) -> PathBuf {
        self.root_dir().join(Self::static_dir_name())
    }

    pub fn index_file(&self) -> PathBuf {
        self.static_dir().join(Self::index_file_name())
    }

    pub fn server_module_file(&self) -> PathBuf {
        self.root_dir().join(&format!("{}_server.wasm", self.name()))
    }

    pub fn is_loaded(&self) -> bool {
        self.instance.is_some()
    }

    pub fn instance_mut(&mut self) -> Option<&mut LappInstance> {
        self.instance.as_mut()
    }

    pub fn take_instance(&mut self) -> Option<LappInstance> {
        self.instance.take()
    }

    pub fn service_sender(&self) -> Option<service::lapp::Sender> {
        self.service_sender.clone()
    }

    pub fn process_http(&mut self, request: Request) -> ServerResult<Response> {
        match self.instance_mut() {
            Some(instance) => Ok(instance.process_http(request)?),
            None => Err(ServerError::LappNotLoaded(self.name().to_string())),
        }
    }

    pub async fn service_stop(&mut self) -> bool {
        if let Some(sender) = self.service_sender.take() {
            service::LappService::stop(sender).await
        } else {
            false
        }
    }

    pub async fn instantiate(&mut self, http_client: Client) -> ServerResult<()> {
        let wasm_bytes = fs::read(self.server_module_file()).await?;

        let mut store = Store::default();
        let module = Module::new(&store, &wasm_bytes)?;

        let is_allow_read = self.is_allowed_permission(Permission::FileRead);
        let is_allow_write = self.is_allowed_permission(Permission::FileWrite);
        let is_allow_db_access = self.is_allowed_permission(Permission::Database);
        let is_allow_http = self.is_allowed_permission(Permission::Http);
        let is_allow_sleep = self.is_allowed_permission(Permission::Sleep);

        let dir_path = self.root_dir().join("data");
        if !dir_path.exists() && (is_allow_read || is_allow_write) {
            fs::create_dir(&dir_path).await?;
        }

        let mut wasi_env = None;
        let mut database_env = None;
        let mut http_env = None;

        let mut imports = if self
            .required_permissions()
            .any(|permission| permission == Permission::FileRead || permission == Permission::FileWrite)
        {
            let env = WasiState::new(self.name())
                .preopen(|preopen| {
                    preopen
                        .directory(&dir_path)
                        .alias("/")
                        .read(is_allow_read)
                        .write(is_allow_write)
                        // todo: why this works always as true?
                        .create(is_allow_write)
                })?
                .finalize(&mut store)?;

            let imports = env.import_object(&mut store, &module)?;
            wasi_env = Some(env);
            imports
        } else {
            Imports::default()
        };

        let mut exports = Exports::new();

        if is_allow_db_access {
            let database_path = self.get_database_path();
            let connection = Arc::new(Mutex::new(Connection::open(database_path)?));

            let env = FunctionEnv::new(&mut store, DatabaseEnv {
                memory_data: None,
                connection: connection.clone(),
            });
            let execute_fn = Function::new_typed_with_env(&mut store, &env, database::execute);
            let query_fn = Function::new_typed_with_env(&mut store, &env, database::query);
            let query_row_fn = Function::new_typed_with_env(&mut store, &env, database::query_row);

            exports.insert("db_execute", execute_fn);
            exports.insert("db_query", query_fn);
            exports.insert("db_query_row", query_row_fn);
            database_env = Some(env);
        }

        if is_allow_http {
            let env = FunctionEnv::new(&mut store, HttpEnv {
                memory_data: None,
                client: http_client,
                settings: self.lapp.settings().network().http().clone(),
            });
            let invoke_http_fn = Function::new_typed_with_env(&mut store, &env, http::invoke_http);

            exports.insert("invoke_http", invoke_http_fn);
            http_env = Some(env);
        }

        if is_allow_sleep {
            let invoke_sleep_fn = Function::new_typed(&mut store, sleep::invoke_sleep);
            exports.insert("invoke_sleep", invoke_sleep_fn);
        }

        imports.register_namespace("env", exports);
        let instance = Instance::new(&mut store, &module, &imports)?;
        let memory_management = MemoryManagementHostData::from_exports(&instance.exports, &store)?;

        if let Some(env) = wasi_env {
            env.data_mut(&mut store).set_memory(memory_management.memory().clone());
        }

        if let Some(env) = database_env {
            env.as_mut(&mut store).memory_data = Some(memory_management.clone());
        }

        if let Some(env) = http_env {
            env.as_mut(&mut store).memory_data = Some(memory_management.clone());
        }

        if let Ok(initialize) = instance.exports.get_function("_initialize") {
            initialize.call(&mut store, &[])?;
        }

        if let Ok(start) = instance.exports.get_function("_start") {
            start.call(&mut store, &[])?;
        }

        if let Ok(init) = instance.exports.get_function("init") {
            let slice = init.typed::<(), u64>(&store)?.call(&mut store)?;
            let mut memory_manager = memory_management.to_manager(&mut store);
            let bytes = unsafe {
                memory_manager
                    .wasm_slice_to_vec(slice)
                    .map_err(LappInstanceError::MemoryManagementError)?
            };
            Result::<(), String>::try_from_slice(&bytes)?.map_err(ServerError::LappInitError)?;
        }

        self.instance.replace(LappInstance {
            instance,
            memory_management,
            store,
        });
        Ok(())
    }

    pub fn update(&mut self, mut query: UpdateQuery) -> LappSettingsResult<UpdateQuery> {
        if let Some(enabled) = query.enabled {
            if self.enabled() != enabled {
                self.set_enabled(enabled);
            } else {
                query.enabled = None;
            }
        }

        if let Some(permission) = query.allow_permission {
            if !self.allow_permission(permission) {
                query.allow_permission = None;
            }
        }

        if let Some(permission) = query.deny_permission {
            if !self.deny_permission(permission) {
                query.deny_permission = None;
            }
        }

        self.save_settings()?;
        Ok(query)
    }

    pub fn check_enabled_and_allow_permissions(&self, permissions: &[Permission]) -> ServerResult<()> {
        if !self.enabled() {
            return Err(ServerError::LappNotEnabled(self.name().into()));
        };
        for &permission in permissions {
            if !self.is_allowed_permission(permission) {
                return Err(ServerError::LappPermissionDenied(self.name().into(), permission));
            }
        }
        Ok(())
    }

    fn get_database_path(&self) -> PathBuf {
        let database_path = self.settings().database().path();

        if database_path.is_relative() {
            self.root_dir().join(database_path)
        } else {
            database_path.into()
        }
    }
}

#[derive(Clone, Deref)]
pub struct SharedLapp(Arc<RwLock<Lapp>>);

impl SharedLapp {
    pub fn new(lapp: Lapp) -> Self {
        Self(Arc::new(RwLock::new(lapp)))
    }

    pub async fn run_service_if_needed(&self) -> ServerResult<service::lapp::Sender> {
        if let Some(sender) = self.read().await.service_sender() {
            Ok(sender)
        } else {
            let (service, sender) = service::LappService::new(self.clone());
            actix::spawn(service.run());

            self.write().await.service_sender = Some(sender.clone());
            Ok(sender)
        }
    }
}
