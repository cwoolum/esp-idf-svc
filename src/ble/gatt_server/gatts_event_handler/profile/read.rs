use core::cmp::min;
use std::collections::HashMap;

use log::debug;

use crate::ble::gatt_server::Profile;
use crate::ble::utilities::AttributeControl;
use crate::private::mutex::{Mutex, RawMutex};
use esp_idf_sys::*;

type Singleton<T> = Mutex<Option<Box<T>>>;

pub(crate) static MESSAGE_CACHE: Singleton<HashMap<u16, Vec<u8>>> =
    Mutex::wrap(RawMutex::new(), None);

const RESPONSE_LENGTH: usize = 600;
// TODO: Pull current MTU size from MTU update events
const MAX_CHUNK_SIZE: u16 = 22;

impl Profile {
    pub(crate) fn on_read(
        &mut self,
        gatts_if: esp_gatt_if_t,
        param: esp_ble_gatts_cb_param_t_gatts_read_evt_param,
    ) {
        for service in &self.services {
            service
                .read()
                .unwrap()
                .characteristics
                .iter()
                .for_each(|characteristic| {
                    let read_char = characteristic.read().unwrap();
                    if read_char.attribute_handle == Some(param.handle) {
                        debug!("Received read event for characteristic {}.", read_char);

                        // If the characteristic has a read handler, call it.
                        if let AttributeControl::ResponseByApp(callback) = &read_char.control {
                            let value;
                            let mut locked_cache = MESSAGE_CACHE.lock();
                            let locked_cache = locked_cache.as_mut().unwrap();

                            let cached_message = locked_cache.get(&param.handle);

                            match cached_message {
                                Some(message) if param.offset > 0 => {
                                    value = message.to_vec();
                                }
                                _ => {
                                    value = callback(param);
                                }
                            }

                            if cached_message.is_none() && value.len() > MAX_CHUNK_SIZE.into() {
                                locked_cache.insert(param.handle, value.to_vec());
                            }

                            let possible_max =
                                min(value.len(), (param.offset + MAX_CHUNK_SIZE).into());
                            let sub_string = &value[param.offset.into()..possible_max.into()];

                            // Remove from the cache once we don't need fragmenting anymore.
                            if sub_string.len() < MAX_CHUNK_SIZE.into() {
                                debug!("Removing from cache {:?} {:?}", param.offset, param.handle);
                                locked_cache.remove(&param.handle);
                            }

                            drop(locked_cache);

                            let mut response = [0u8; RESPONSE_LENGTH];
                            response[..sub_string.len()].copy_from_slice(sub_string);

                            let mut esp_rsp = esp_gatt_rsp_t {
                                attr_value: esp_gatt_value_t {
                                    auth_req: 0,
                                    handle: param.handle,
                                    len: min(value.len() as u16 - param.offset, MAX_CHUNK_SIZE),
                                    offset: param.offset,
                                    value: response,
                                },
                            };

                            unsafe {
                                esp_nofail!(esp_ble_gatts_send_response(
                                    gatts_if,
                                    param.conn_id,
                                    param.trans_id,
                                    // TODO: Allow different statuses.
                                    esp_gatt_status_t_ESP_GATT_OK,
                                    &mut esp_rsp
                                ));
                            }
                        }
                    } else {
                        read_char.descriptors.iter().for_each(|descriptor| {
                            let read_desc = descriptor.read().unwrap();
                            debug!(
                                "MCC: Checking descriptor {} ({:?}).",
                                read_desc, read_desc.attribute_handle
                            );

                            if read_desc.attribute_handle == Some(param.handle) {
                                debug!("Received read event for descriptor {}.", read_desc);

                                if let AttributeControl::ResponseByApp(callback) =
                                    &read_desc.control
                                {
                                    let value = callback(param);

                                    let mut response = [0u8; RESPONSE_LENGTH];
                                    response[..value.len()].copy_from_slice(&value);

                                    let mut esp_rsp = esp_gatt_rsp_t {
                                        attr_value: esp_gatt_value_t {
                                            auth_req: 0,
                                            handle: param.handle,
                                            len: value.len() as u16,
                                            offset: 0,
                                            value: response,
                                        },
                                    };

                                    unsafe {
                                        esp_nofail!(esp_ble_gatts_send_response(
                                            gatts_if,
                                            param.conn_id,
                                            param.trans_id,
                                            esp_gatt_status_t_ESP_GATT_OK,
                                            &mut esp_rsp
                                        ));
                                    }
                                }
                            }
                        });
                    }
                });
        }
    }
}