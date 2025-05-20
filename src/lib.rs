use btleplug::api::{
    BDAddr, Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _,
    ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use btleplug::Error as BleError;
use btleplug::{Error, Result as BleResult};
use futures::StreamExt;
use std::collections::{BTreeSet, HashMap};
use std::ffi::{c_char, c_int, CString};
use std::mem::size_of;
use std::ptr::{null, null_mut};
use std::slice::from_raw_parts;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::sync::Mutex;
use uuid::Uuid;

use log::{debug, error, info, trace, warn, LevelFilter};

const SUCCESS: c_int = 0;
const ERROR_FAIL: c_int = 1;
const INVALID_ARGUMENT: c_int = 2;
const ERROR_PERMISSION_DENIED: c_int = 101;
const ERROR_DEVICE_NOT_FOUND: c_int = 102;
const ERROR_NOT_CONNECTED: c_int = 103;
const ERROR_UNEXPECTED_CALLBACK: c_int = 104;
const ERROR_UNEXPECTED_CHARACTERISTIC: c_int = 105;
const ERROR_NO_SUCH_CHARACTERISTIC: c_int = 106;
const ERROR_NOT_SUPPORTED: c_int = 107;
const ERROR_TIMED_OUT: c_int = 108;
const ERROR_UUID: c_int = 109;
const ERROR_INVALID_BD_ADDR: c_int = 110;
const ERROR_RUNTIME_ERROR: c_int = 111;

type PeripheralFoundCallback = extern "C" fn(
    id: u64,
    peripheral: *mut CPeripheral,
    services: *const Uuid,
    service_count: c_int,
) -> c_int;
type PeripheralEventCallback = extern "C" fn(id: u64);
type CompletedCallback = extern "C" fn(result: c_int);

fn set_error_string(module: &*mut CModule, str: CString) {
    unsafe {
        *(**module).module.last_error.blocking_lock() = str;
    }
}

fn set_error_str(module: &*mut CModule, str: &str) {
    unsafe {
        *(**module).module.last_error.blocking_lock() = CString::new(str).unwrap();
    }
}

fn set_error(module: &*mut CModule, err: &Error) {
    unsafe {
        *(**module).module.last_error.blocking_lock() = error_into_cstring(err);
    }
}

fn set_peripheral_error_str(peripheral: &*mut CPeripheral, str: &str) {
    unsafe {
        *(**peripheral).p.last_error.blocking_lock() = CString::new(str).unwrap();
    }
}

struct ModuleInt {
    last_error: Mutex<CString>,
    runtime: Option<Runtime>,
    adapter: Option<Adapter>,
}

pub struct CModule {
    module: Arc<ModuleInt>,
}

impl CModule {
    fn new(runtime: Option<Runtime>, adapter: Option<Adapter>) -> CModule {
        CModule {
            module: Arc::new(ModuleInt {
                runtime,
                adapter,
                last_error: Mutex::new(CString::default()),
            }),
        }
    }
}

struct PeripheralHandle {
    peripheral: Peripheral,
    services: Vec<Uuid>,
    last_error: Mutex<CString>,
}

pub struct CPeripheral {
    module: Arc<ModuleInt>,
    p: Arc<PeripheralHandle>,
}

#[repr(C)]
pub struct ServiceDescriptors {
    service_count: c_int,
}

#[repr(C)]
pub struct ServiceDescriptor {
    uuid: Uuid,
    characteristic_count: c_int,
}
#[repr(C)]
pub struct CharacteristicDescriptor {
    uuid: Uuid,
    properties: CharPropFlags,
    descriptor_count: c_int,
}

#[repr(C)]
pub struct CharacteristicDescriptorDescriptor {
    uuid: Uuid,
}

impl CPeripheral {
    fn new(module: Arc<ModuleInt>, peripheral: Peripheral, services: Vec<Uuid>) -> CPeripheral {
        CPeripheral {
            module,
            p: Arc::new(PeripheralHandle {
                peripheral,
                services,
                last_error: Mutex::new(CString::default()),
            }),
        }
    }
}

async fn get_central(manager: &Manager) -> BleResult<Adapter> {
    let adapters = manager.adapters().await?;
    match adapters.into_iter().nth(0) {
        None => Err(BleError::RuntimeError(String::from("No adapters found"))),
        Some(a) => Ok(a),
    }
}

async fn get_manager() -> BleResult<Adapter> {
    let manager = Manager::new().await?;
    get_central(&manager).await
}

unsafe fn error_into_cstring(e: &Error) -> CString {
    CString::new(e.to_string()).unwrap_or(CString::new("Unknown error").unwrap())
}

unsafe fn error_to_result(e: &Error) -> c_int {
    match e {
        Error::PermissionDenied => ERROR_PERMISSION_DENIED,
        Error::DeviceNotFound => ERROR_DEVICE_NOT_FOUND,
        Error::NotConnected => ERROR_NOT_CONNECTED,
        Error::UnexpectedCallback => ERROR_UNEXPECTED_CALLBACK,
        Error::UnexpectedCharacteristic => ERROR_UNEXPECTED_CHARACTERISTIC,
        Error::NoSuchCharacteristic => ERROR_NO_SUCH_CHARACTERISTIC,
        Error::NotSupported(_) => ERROR_NOT_SUPPORTED,
        Error::TimedOut(_) => ERROR_TIMED_OUT,
        Error::Uuid(_) => ERROR_UUID,
        Error::InvalidBDAddr(_) => ERROR_INVALID_BD_ADDR,
        Error::RuntimeError(_) => ERROR_RUNTIME_ERROR,
        Error::Other(_) => ERROR_FAIL,
    }
}

unsafe fn get_long_addr(a: BDAddr) -> u64 {
    let addr = a.into_inner();
    let mut lbytes = [0u8; 8];
    lbytes[2..].copy_from_slice(&addr);
    u64::from_be_bytes(lbytes)
}

#[no_mangle]
pub extern "C" fn set_log_level(level: c_int) {
    simple_logging::log_to_stderr(match level {
        0 => LevelFilter::Off,
        1 => LevelFilter::Error,
        2 => LevelFilter::Warn,
        3 => LevelFilter::Info,
        4 => LevelFilter::Debug,
        5 => LevelFilter::Trace,
        _ => LevelFilter::Off,
    });
}

#[no_mangle]
pub unsafe extern "C" fn create_module(module: *mut *mut CModule) -> c_int {
    trace!("Enter: create_module");
    *module = null_mut();

    let runtime = match Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to initialize tokio::Runtime {:?}", e);
            *module = Box::into_raw(Box::new(CModule::new(None, None)));
            set_error_string(&*module, CString::new(e.to_string()).unwrap());
            return ERROR_FAIL;
        }
    };

    debug!("Initializing adapter with runtime");
    let adapter = match runtime.block_on(get_manager()) {
        Ok(a) => a,
        Err(e) => {
            warn!("Failed to initialize Adapter {:?}", e);
            *module = Box::into_raw(Box::new(CModule::new(Some(runtime), None)));
            set_error(&*module, &e);
            return error_to_result(&e);
        }
    };

    trace!("Success: create_module");
    *module = Box::into_raw(Box::new(CModule::new(Some(runtime), Some(adapter))));
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn set_event_callbacks(
    module: *mut CModule,
    found: PeripheralFoundCallback,
    disconnected: PeripheralEventCallback,
) -> c_int {
    trace!("Enter: set_event_callbacks");
    if module.is_null() {
        error!("null module");
        return INVALID_ARGUMENT;
    }

    let m = &(*module).module;
    if m.adapter.is_none() || m.runtime.is_none() {
        error!("null adapter/runtime");
        set_error_str(&module, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();

    let m = (*module).module.clone();

    runtime.spawn(async move {
        let adapter = m.adapter.as_ref().unwrap();
        let mut events = adapter.events().await?;
        let weak = Arc::downgrade(&m);
        drop(m);

        debug!("Starting scan");
        let mut device_map = HashMap::new();

        while let Some(event) = events.next().await {
            match event {
                CentralEvent::DeviceDiscovered(id) => {
                    debug!("Device discovered: {:?}", id);
                    let l_mod = match weak.upgrade() {
                        None => {
                            break;
                        }
                        Some(a) => a,
                    };
                    let adapter = l_mod.adapter.as_ref().unwrap();
                    match adapter.peripheral(&id).await {
                        Ok(p) => {
                            info!("Sending peripheral {:?}", id);
                            let raw = Box::into_raw(Box::new(CPeripheral::new(
                                Arc::clone(&l_mod),
                                p,
                                Vec::default(),
                            )));
                            let addr = get_long_addr((*raw).p.peripheral.address());
                            device_map.insert(id, addr);
                            if 0 == found(addr, raw, null(), 0) {
                                // The handle was rejected, drop it
                                free_ptr(raw);
                            }
                        }
                        Err(e) => {
                            error!("Failed to find discovered device for {:#}, {:?}", id, e);
                        }
                    }
                }
                CentralEvent::ServicesAdvertisement { id, services } => {
                    debug!("Services discovered: {:?} : {:?}", id, services);
                    let l_mod = match weak.upgrade() {
                        None => {
                            break;
                        }
                        Some(a) => a,
                    };
                    let adapter = l_mod.adapter.as_ref().unwrap();
                    match adapter.peripheral(&id).await {
                        Ok(p) => {
                            let raw = Box::into_raw(Box::new(CPeripheral::new(
                                Arc::clone(&l_mod),
                                p,
                                Vec::default(),
                            )));
                            let addr = get_long_addr((*raw).p.peripheral.address());
                            device_map.insert(id, addr);
                            if 0 == found(
                                addr,
                                raw,
                                (*raw).p.services.as_ptr(),
                                (*raw).p.services.len() as c_int,
                            ) {
                                // The handle was rejected, drop it
                                free_ptr(raw);
                            }
                        }
                        Err(e) => {
                            error!("Failed to find discovered device for {:#}, {:?}", id, e);
                        }
                    }
                }
                CentralEvent::DeviceDisconnected(id) => {
                    info!("Device disconnected : {:?}", id);
                    match device_map.get(&id) {
                        Some(addr) => {
                            disconnected(*addr);
                        }
                        None => {
                            warn!("Disconnect from unrecognized peripheral: {:?}", id);
                        }
                    }
                }
                _ => {}
            }
        }
        info!("Event listening ended!");
        Ok::<(), BleError>(())
    });

    trace!("Success: set_event_callbacks");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn start_scan_peripherals(
    module: *mut CModule,
    service_uuids: *mut Uuid,
    service_uuid_count: i32,
) -> c_int {
    trace!("Enter: peripheral_is_connected");
    if module.is_null() {
        error!("null module");
        return INVALID_ARGUMENT;
    }

    let m = &(*module).module;
    if m.adapter.is_none() || m.runtime.is_none() {
        error!("null adapter/runtime");
        set_error_str(&module, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();
    let adapter = m.adapter.as_ref().unwrap();

    let filter = match service_uuid_count {
        0 => {
            debug!("No filters applied");
            ScanFilter::default()
        }
        1..=100 => {
            if service_uuids.is_null() {
                set_error_str(&module, "Null argument: service_uuids");
                return INVALID_ARGUMENT;
            }

            let mut v = Vec::new();
            for i in 0..service_uuid_count {
                v.push(*service_uuids.offset(i as isize));
            }

            debug!("Applying filters to scan: {:?}", v);
            ScanFilter { services: v }
        }
        _ => {
            error!("Invalid number of filters provided: {service_uuid_count}");
            set_error_str(
                &module,
                "Out of range: service_uuid_count must be in range 1..100",
            );
            return ERROR_FAIL;
        }
    };

    match runtime.block_on(adapter.start_scan(filter)) {
        Ok(_) => {
            trace!("Success: start_scan_peripherals");
            SUCCESS
        }
        Err(e) => {
            error!("Error start_scan: {:?}", e);
            set_error(&module, &e);
            error_to_result(&e)
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn stop_scan_peripherals(module: *mut CModule) -> c_int {
    trace!("Enter: stop_scan_peripherals");
    if module.is_null() {
        error!("null module");
        return INVALID_ARGUMENT;
    }

    let m = &(*module).module;

    if m.adapter.is_none() || m.runtime.is_none() {
        error!("null adapter/runtime");
        set_error_str(&module, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();
    let adapter = m.adapter.as_ref().unwrap();

    match runtime.block_on(adapter.stop_scan()) {
        Err(e) => {
            error!("error in stop_scan: {:?}", e);
            set_error(&module, &e);
            return error_to_result(&e);
        }
        _ => {}
    };

    trace!("Success: stop_scan_peripherals");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_get_id(
    peripheral: *mut CPeripheral,
    id: *mut *const c_char,
) -> c_int {
    if peripheral.is_null() {
        *id = null();
        return INVALID_ARGUMENT;
    }

    let p = &(*peripheral).p;
    let id_str = CString::new(p.peripheral.id().to_string()).unwrap();
    *id = id_str.into_raw();
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_get_address(
    peripheral: *mut CPeripheral,
    address: *mut u64,
) -> c_int {
    if peripheral.is_null() {
        *address = 0;
        return INVALID_ARGUMENT;
    }

    let p = &(*peripheral).p;
    *address = get_long_addr(p.peripheral.address());
    SUCCESS
}

type IsConnectedCallback = extern "C" fn(result: c_int, connected: c_int);

#[no_mangle]
pub unsafe extern "C" fn peripheral_is_connected(
    peripheral: *mut CPeripheral,
    completed_callback: IsConnectedCallback,
) -> c_int {
    trace!("Enter: peripheral_is_connected");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();

    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap.peripheral.is_connected().await {
            Ok(v) => {
                debug!("Connected: {v}");
                completed_callback(SUCCESS, c_int::from(v));
            }
            Err(e) => {
                error!("Error calling is_connected: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e), 0);
            }
        }
    });

    trace!("Success: peripheral_is_connected");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_connect(
    peripheral: *mut CPeripheral,
    completed_callback: CompletedCallback,
) -> c_int {
    trace!("Enter: peripheral_connect");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();

    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap.peripheral.connect().await {
            Ok(()) => {
                debug!("Connected");
                completed_callback(SUCCESS);
            }
            Err(e) => {
                error!("Error calling connect: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e));
            }
        }
    });
    trace!("Success: peripheral_connect");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_disconnect(
    peripheral: *mut CPeripheral,
    completed_callback: CompletedCallback,
) -> c_int {
    trace!("Enter: peripheral_disconnect");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();

    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap.peripheral.disconnect().await {
            Ok(()) => {
                debug!("Disconnected");
                completed_callback(SUCCESS);
            }
            Err(e) => {
                error!("Error calling disconnect: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e));
            }
        }
    });
    trace!("Success: peripheral_disconnect");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_discover_services(
    peripheral: *mut CPeripheral,
    completed_callback: CompletedCallback,
) -> c_int {
    trace!("Enter: peripheral_discover_services");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();

    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap.peripheral.discover_services().await {
            Ok(()) => {
                debug!("Disconnected");
                completed_callback(SUCCESS);
            }
            Err(e) => {
                error!("Error calling discover_services: {:#?}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e));
            }
        }
    });

    trace!("Success: peripheral_discover_services");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_get_services(
    peripheral: *mut CPeripheral,
    service_descriptors: *mut *mut u8,
) -> c_int {
    trace!("Enter: peripheral_discover_services");

    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let p = &(*peripheral).p;
    let services = p.peripheral.services();
    info!("Found {} services for peripheral", services.len());

    let mut buffer = Vec::new();
    buffer.extend([0u8; 8]);
    buffer.extend(any_as_u8_slice(&ServiceDescriptors {
        service_count: services.len() as c_int,
    }));
    for s in services {
        let sd = ServiceDescriptor {
            uuid: s.uuid,
            characteristic_count: s.characteristics.len() as c_int,
        };
        buffer.extend(any_as_u8_slice(&sd));
        for c in s.characteristics {
            let cd = CharacteristicDescriptor {
                uuid: c.uuid,
                properties: c.properties,
                descriptor_count: c.descriptors.len() as c_int,
            };
            buffer.extend(any_as_u8_slice(&cd));
            for d in c.descriptors {
                buffer.extend(any_as_u8_slice(&d.uuid));
            }
        }
    }

    unsafe fn any_as_u8_slice<T: Sized>(p: &T) -> &[u8] {
        from_raw_parts((p as *const T) as *const u8, size_of::<T>())
    }

    let size = buffer.len();
    let cap = buffer.capacity();
    let raw = buffer.as_mut_ptr();
    buffer.leak();

    let i_raw = raw as *mut u32;
    *i_raw = size as u32;
    *i_raw.offset(1) = cap as u32;

    *service_descriptors = raw;
    trace!("Success: peripheral_get_services");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn free_peripheral_services(services: *mut *mut u8) -> c_int {
    let i_raw = services as *mut u32;
    let size = *i_raw;
    let cap = *i_raw.offset(1);
    let _ = Vec::from_raw_parts(services, size as usize, cap as usize);
    SUCCESS
}

type NotifyCallback = extern "C" fn(uuid: Uuid, data: *const u8, data_length: c_int);

#[no_mangle]
pub unsafe extern "C" fn peripheral_register_notification_events(
    peripheral: *mut CPeripheral,
    ready: CompletedCallback,
    notify_callback: NotifyCallback,
) -> c_int {
    trace!("Enter: peripheral_register_notification_events");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    let runtime = m.runtime.as_ref().unwrap();
    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap.peripheral.notifications().await {
            Ok(mut n) => {
                debug!("Notifications listening");
                ready(SUCCESS);
                while let Some(data) = n.next().await {
                    info!("Received {} bytes on {}", data.value.len(), data.uuid);
                    notify_callback(data.uuid, data.value.as_ptr(), data.value.len() as c_int)
                }
            }
            Err(e) => {
                error!("Error calling connect: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                ready(error_to_result(&e));
            }
        }
    });

    trace!("Success: peripheral_register_notification_events");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_subscribe(
    peripheral: *mut CPeripheral,
    service_uuid: Uuid,
    uuid: Uuid,
    completed_callback: CompletedCallback,
) -> c_int {
    trace!("Enter: peripheral_subscribe");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    info!("Subscribing notification for {service_uuid}:{uuid}");
    let runtime = m.runtime.as_ref().unwrap();
    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap
            .peripheral
            .subscribe(&Characteristic {
                service_uuid,
                uuid,
                descriptors: BTreeSet::default(),
                properties: CharPropFlags::empty(),
            })
            .await
        {
            Ok(()) => {
                debug!("Notifications subscribed");
                completed_callback(SUCCESS)
            }
            Err(e) => {
                error!("Error calling connect: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e));
            }
        }
    });
    trace!("Success: peripheral_subscribe");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_unsubscribe(
    peripheral: *mut CPeripheral,
    service_uuid: Uuid,
    uuid: Uuid,
    completed_callback: CompletedCallback,
) -> c_int {
    trace!("Enter: peripheral_unsubscribe");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    info!("Unsubscribing notification for {service_uuid}:{uuid}");
    let runtime = m.runtime.as_ref().unwrap();
    let ap = (*peripheral).p.clone();
    runtime.spawn(async move {
        match ap
            .peripheral
            .unsubscribe(&Characteristic {
                service_uuid,
                uuid,
                descriptors: BTreeSet::default(),
                properties: CharPropFlags::empty(),
            })
            .await
        {
            Ok(()) => {
                debug!("Notifications Unsubscribed");
                completed_callback(SUCCESS)
            }
            Err(e) => {
                error!("Error calling connect: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e));
            }
        }
    });
    trace!("Success: peripheral_unsubscribe");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_write(
    peripheral: *mut CPeripheral,
    service_uuid: Uuid,
    uuid: Uuid,
    with_response: bool,
    data: *mut u8,
    data_length: u32,
    completed_callback: CompletedCallback,
) -> c_int {
    trace!("Enter: peripheral_write");
    if peripheral.is_null() {
        error!("null peripheral handle");
        return INVALID_ARGUMENT;
    }
    if data.is_null() {
        error!("null data");
        set_peripheral_error_str(&peripheral, "Null argument: data");
        return INVALID_ARGUMENT;
    }

    let m = &(*peripheral).module;

    if m.runtime.is_none() {
        error!("null runtime handle");
        set_peripheral_error_str(&peripheral, "Invalid module");
        return INVALID_ARGUMENT;
    }

    info!("Writing {data_length} bytes to {service_uuid}:{uuid} (with_response: {with_response})");
    let runtime = m.runtime.as_ref().unwrap();
    let ap = (*peripheral).p.clone();
    let data_arr = from_raw_parts(data, data_length as usize);
    runtime.spawn(async move {
        let characteristic = Characteristic {
            service_uuid,
            uuid,
            descriptors: BTreeSet::default(),
            properties: CharPropFlags::empty(),
        };
        let write_type = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        match ap
            .peripheral
            .write(&characteristic, data_arr, write_type)
            .await
        {
            Ok(()) => {
                debug!("Data written");
                completed_callback(SUCCESS)
            }
            Err(e) => {
                error!("Error calling write: {:#}", e);
                *ap.last_error.lock().await = error_into_cstring(&e);
                completed_callback(error_to_result(&e));
            }
        }
    });
    trace!("Success: peripheral_write");
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn get_last_module_error(module: *mut CModule) -> *const c_char {
    if module.is_null() {
        return null();
    }

    unsafe { (*module).module.last_error.blocking_lock().as_ptr() }
}

#[no_mangle]
pub unsafe extern "C" fn peripheral_get_last_error(peripheral: *mut CPeripheral) -> *const c_char {
    if peripheral.is_null() {
        return null();
    }

    unsafe { (*peripheral).p.last_error.blocking_lock().as_ptr() }
}

unsafe fn free_ptr<T>(handle: *mut T) -> c_int {
    if handle.is_null() {
        return SUCCESS;
    }
    let _ = Box::from_raw(handle);
    SUCCESS
}

#[no_mangle]
pub unsafe extern "C" fn free_module(module: *mut CModule) -> c_int {
    free_ptr(module)
}

#[no_mangle]
pub unsafe extern "C" fn free_peripheral(peripheral: *mut CPeripheral) -> c_int {
    free_ptr(peripheral)
}

#[no_mangle]
pub unsafe extern "C" fn free_string(s: *mut c_char) -> c_int {
    if s.is_null() {
        return SUCCESS;
    }

    let _ = CString::from_raw(s);
    SUCCESS
}

#[cfg(test)]
mod tests {
    #[test]
    fn it_works() {}
}
