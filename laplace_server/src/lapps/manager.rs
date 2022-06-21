use std::{
    collections::HashMap,
    fs, io,
    path::PathBuf,
    sync::{RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use log::{error, info};
use wasmer::Instance;

use crate::{
    error::{ServerError, ServerResult},
    Lapp,
};

pub struct LappsManager {
    lapps: HashMap<String, RwLock<Lapp>>,
    lapps_path: PathBuf,
    http_client: reqwest::blocking::Client,
}

impl LappsManager {
    pub fn new(lapps_path: impl Into<PathBuf>) -> io::Result<Self> {
        let lapps_path = lapps_path.into();
        fs::read_dir(&lapps_path)?
            .map(|entry| {
                entry.and_then(|dir| {
                    let name = dir.file_name().into_string().map_err(|invalid_name| {
                        error!("Lapp name '{:?}' is not valid UTF-8", invalid_name);
                        io::Error::from(io::ErrorKind::InvalidData)
                    })?;
                    Ok((name.clone(), RwLock::new(Lapp::new(name, dir.path()))))
                })
            })
            .collect::<io::Result<_>>()
            .map(|lapps| {
                let http_client = reqwest::blocking::Client::new();
                Self {
                    lapps,
                    lapps_path,
                    http_client,
                }
            })
    }

    pub fn insert_lapp(&mut self, lapp_name: impl Into<String>) {
        let lapp_name = lapp_name.into();
        let root_dir = self.lapps_path.join(&lapp_name);
        self.lapps
            .insert(lapp_name.clone(), RwLock::new(Lapp::new(lapp_name, root_dir)));
    }

    pub fn load(&self, mut lapp: RwLockWriteGuard<'_, Lapp>) -> ServerResult<()> {
        let http_client = self.http_client.clone();
        lapp.instantiate(http_client)
    }

    pub async fn unload(&self, mut lapp: RwLockWriteGuard<'_, Lapp>) -> ServerResult<()> {
        lapp.take_instance();
        lapp.service_stop().await;

        Ok(())
    }

    pub fn load_lapps(&self) {
        let http_client = self.http_client.clone();
        for (name, lapp_lock) in &self.lapps {
            let lapp = lapp_lock.read().expect("Lapp is not readable");
            if !lapp.is_main() && lapp.enabled() && !lapp.is_loaded() {
                info!("Load lapp '{}'", name);

                drop(lapp);
                lapp_lock
                    .write()
                    .expect("Lapp is not writable")
                    .instantiate(http_client.clone())
                    .expect("Lapp should be loaded");
            }
        }
    }

    pub fn lapp_dir(&self, lapp_name: impl AsRef<str>) -> PathBuf {
        self.lapps_path.join(lapp_name.as_ref())
    }

    pub fn is_loaded(&self, lapp_name: impl AsRef<str>) -> bool {
        self.lapp(lapp_name.as_ref())
            .map(|lapp| lapp.is_loaded())
            .unwrap_or(false)
    }

    pub fn loaded_lapp(&self, lapp_name: impl AsRef<str>) -> ServerResult<(RwLockReadGuard<Lapp>, Instance)> {
        let lapp_name = lapp_name.as_ref();
        self.lapp(lapp_name).and_then(|lapp| {
            lapp.instance()
                .ok_or_else(|| ServerError::LappNotLoaded(lapp_name.to_string()))
                .map(|instance| (lapp, instance))
        })
    }

    pub fn lapp(&self, lapp_name: impl AsRef<str> + ToString) -> ServerResult<RwLockReadGuard<Lapp>> {
        self.lapps
            .get(lapp_name.as_ref())
            .ok_or_else(|| ServerError::LappNotFound(lapp_name.to_string()))
            .and_then(|lapp| lapp.read().map_err(|_| ServerError::LappNotLock))
    }

    pub fn lapp_mut(&self, lapp_name: impl AsRef<str> + ToString) -> ServerResult<RwLockWriteGuard<Lapp>> {
        self.lapps
            .get(lapp_name.as_ref())
            .ok_or_else(|| ServerError::LappNotFound(lapp_name.to_string()))
            .and_then(|lapp| lapp.write().map_err(|_| ServerError::LappNotLock))
    }

    pub fn lapps_iter(&self) -> impl Iterator<Item = &RwLock<Lapp>> {
        self.lapps.values()
    }

    pub fn instance(&self, lapp_name: impl AsRef<str> + ToString) -> ServerResult<Instance> {
        self.lapp(lapp_name.as_ref())?
            .instance()
            .ok_or_else(|| ServerError::LappNotLoaded(lapp_name.to_string()))
    }
}
