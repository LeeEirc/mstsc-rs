use std::ffi::c_void;
use std::mem::{ManuallyDrop, size_of};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{
    DISP_E_UNKNOWNNAME, E_NOINTERFACE, E_NOTIMPL, FreeLibrary, HMODULE, HWND, LPARAM, RECT, S_OK,
    SIZE, VARIANT_BOOL, WPARAM,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET,
    DISPATCH_PROPERTYPUT, DISPPARAMS, EXCEPINFO, IConnectionPoint, IConnectionPointContainer,
    IDispatch, IDispatch_Impl, IPersistStreamInit, ITypeInfo,
};
use windows::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::Win32::System::Ole::{
    DISPID_PROPERTYPUT, IOleClientSite, IOleClientSite_Impl, IOleContainer,
    IOleInPlaceActiveObject, IOleInPlaceFrame, IOleInPlaceFrame_Impl, IOleInPlaceObject,
    IOleInPlaceSite, IOleInPlaceSite_Impl, IOleInPlaceUIWindow, IOleInPlaceUIWindow_Impl,
    IOleObject, IOleWindow_Impl, OLECLOSE_NOSAVE, OLEGETMONIKER, OLEINPLACEFRAMEINFO,
    OLEIVERB_INPLACEACTIVATE, OLEMENUGROUPWIDTHS, OLEWHICHMK, OleSetContainedObject,
};
use windows::Win32::System::Variant::{
    VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_I2, VT_I4, VT_INT, VT_UI2, VT_UI4,
    VT_UINT, VariantClear,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, HACCEL, HMENU, MSG, PostMessageW};
use windows::core::{
    BOOL, BSTR, ComObject, Error as WinError, GUID, HRESULT, IUnknown, IUnknownImpl, Interface,
    OutRef, PCWSTR, Ref,
};

use crate::config::SessionConfig;
use crate::error::{Result, WindowsContext};
use crate::rdp::RdpDocument;

use super::ui::WM_RDP_EVENT;

// Microsoft RDP Client Control (not safe for scripting). Version 10 is the
// desktop control for Windows 10/11; version 9 remains a compatible fallback.
// Unlike CLSID_RemoteDesktopClient, this class is supported in normal desktop
// processes rather than being restricted to a Store/AppContainer host.
const CLSID_MS_RDP_CLIENT_10_NOT_SAFE_FOR_SCRIPTING: GUID =
    GUID::from_u128(0xa0c63c30_f08d_4ab4_907c_34905d770c7d);
const CLSID_MS_RDP_CLIENT_9_NOT_SAFE_FOR_SCRIPTING: GUID =
    GUID::from_u128(0x8b918b82_7985_4c24_89df_c33ad2bbfbcd);
const DIID_MS_TSC_AX_EVENTS: GUID = GUID::from_u128(0x336d5562_efa8_482e_8cb3_c5c0fc7a7db6);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RdpEventKind {
    Connecting,
    Connected,
    LoginCompleted,
    Disconnected,
    DialogDisplaying,
    DialogDismissed,
    NetworkStatusChanged,
    RemoteDesktopSizeChanged,
}

pub(super) struct RdpEvent {
    pub kind: RdpEventKind,
    pub arguments: Vec<String>,
}

#[windows::core::implement(IOleClientSite, IOleInPlaceSite, IOleInPlaceFrame)]
struct ActiveXSite {
    hwnd: HWND,
}

impl ActiveXSite {
    fn client_rect(&self) -> windows::core::Result<RECT> {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut rect) }?;
        Ok(rect)
    }
}

impl IOleClientSite_Impl for ActiveXSite_Impl {
    fn SaveObject(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn GetMoniker(
        &self,
        _dwassign: &OLEGETMONIKER,
        _dwwhichmoniker: &OLEWHICHMK,
    ) -> windows::core::Result<windows::Win32::System::Com::IMoniker> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn GetContainer(&self) -> windows::core::Result<IOleContainer> {
        Err(WinError::from_hresult(E_NOINTERFACE))
    }

    fn ShowObject(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnShowWindow(&self, _fshow: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn RequestNewObjectLayout(&self) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }
}

impl IOleWindow_Impl for ActiveXSite_Impl {
    fn GetWindow(&self) -> windows::core::Result<HWND> {
        Ok(self.hwnd)
    }

    fn ContextSensitiveHelp(&self, _fentermode: BOOL) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }
}

impl IOleInPlaceSite_Impl for ActiveXSite_Impl {
    fn CanInPlaceActivate(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnInPlaceActivate(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnUIActivate(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn GetWindowContext(
        &self,
        ppframe: OutRef<IOleInPlaceFrame>,
        ppdoc: OutRef<IOleInPlaceUIWindow>,
        lprcposrect: *mut RECT,
        lprccliprect: *mut RECT,
        lpframeinfo: *mut OLEINPLACEFRAMEINFO,
    ) -> windows::core::Result<()> {
        let rect = self.client_rect()?;
        unsafe {
            if !lprcposrect.is_null() {
                lprcposrect.write(rect);
            }
            if !lprccliprect.is_null() {
                lprccliprect.write(rect);
            }
            if !lpframeinfo.is_null() {
                lpframeinfo.write(OLEINPLACEFRAMEINFO {
                    cb: size_of::<OLEINPLACEFRAMEINFO>() as u32,
                    fMDIApp: BOOL(0),
                    hwndFrame: self.hwnd,
                    haccel: HACCEL::default(),
                    cAccelEntries: 0,
                });
            }
        }
        ppframe.write(Some(self.to_interface::<IOleInPlaceFrame>()))?;
        ppdoc.write(None)?;
        Ok(())
    }

    fn Scroll(&self, _scrollextant: &SIZE) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn OnUIDeactivate(&self, _fundoable: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnInPlaceDeactivate(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn DiscardUndoState(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn DeactivateAndUndo(&self) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn OnPosRectChange(&self, _lprcposrect: *const RECT) -> windows::core::Result<()> {
        Ok(())
    }
}

impl IOleInPlaceUIWindow_Impl for ActiveXSite_Impl {
    fn GetBorder(&self) -> windows::core::Result<RECT> {
        self.client_rect()
    }

    fn RequestBorderSpace(&self, _pborderwidths: *const RECT) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn SetBorderSpace(&self, _pborderwidths: *const RECT) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetActiveObject(
        &self,
        _pactiveobject: Ref<IOleInPlaceActiveObject>,
        _pszobjname: &PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

impl IOleInPlaceFrame_Impl for ActiveXSite_Impl {
    fn InsertMenus(
        &self,
        _hmenushared: HMENU,
        _lpmenuwidths: *mut OLEMENUGROUPWIDTHS,
    ) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn SetMenu(
        &self,
        _hmenushared: HMENU,
        _holemenu: isize,
        _hwndactiveobject: HWND,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn RemoveMenus(&self, _hmenushared: HMENU) -> windows::core::Result<()> {
        Ok(())
    }

    fn SetStatusText(&self, _pszstatustext: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn EnableModeless(&self, _fenable: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn TranslateAccelerator(&self, _lpmsg: *const MSG, _wid: u16) -> windows::core::Result<()> {
        Err(WinError::from_hresult(HRESULT(1)))
    }
}

#[windows::core::implement(IDispatch)]
struct EventCallback {
    hwnd: HWND,
    events: Arc<Mutex<Vec<RdpEvent>>>,
}

impl IDispatch_Impl for EventCallback_Impl {
    fn GetTypeInfoCount(&self) -> windows::core::Result<u32> {
        Ok(0)
    }

    fn GetTypeInfo(&self, _itinfo: u32, _lcid: u32) -> windows::core::Result<ITypeInfo> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn GetIDsOfNames(
        &self,
        _riid: *const GUID,
        _rgsznames: *const PCWSTR,
        _cnames: u32,
        _lcid: u32,
        _rgdispid: *mut i32,
    ) -> windows::core::Result<()> {
        Err(WinError::from_hresult(DISP_E_UNKNOWNNAME))
    }

    fn Invoke(
        &self,
        _dispidmember: i32,
        _riid: *const GUID,
        _lcid: u32,
        _wflags: DISPATCH_FLAGS,
        pdispparams: *const DISPPARAMS,
        _pvarresult: *mut VARIANT,
        _pexcepinfo: *mut EXCEPINFO,
        _puargerr: *mut u32,
    ) -> windows::core::Result<()> {
        let Some(kind) = legacy_event_kind(_dispidmember) else {
            return Ok(());
        };
        let arguments = unsafe { dispatch_arguments(pdispparams) };
        if let Ok(mut events) = self.events.lock() {
            events.push(RdpEvent { kind, arguments });
            let _ = unsafe { PostMessageW(Some(self.hwnd), WM_RDP_EVENT, WPARAM(0), LPARAM(0)) };
        }
        Ok(())
    }
}

fn legacy_event_kind(dispid: i32) -> Option<RdpEventKind> {
    match dispid {
        1 => Some(RdpEventKind::Connecting),
        2 => Some(RdpEventKind::Connected),
        3 => Some(RdpEventKind::LoginCompleted),
        4 => Some(RdpEventKind::Disconnected),
        12 => Some(RdpEventKind::RemoteDesktopSizeChanged),
        18 => Some(RdpEventKind::DialogDisplaying),
        19 => Some(RdpEventKind::DialogDismissed),
        29 => Some(RdpEventKind::NetworkStatusChanged),
        31 => Some(RdpEventKind::Connected),
        32 => Some(RdpEventKind::Connecting),
        _ => None,
    }
}

unsafe fn dispatch_arguments(params: *const DISPPARAMS) -> Vec<String> {
    if params.is_null() {
        return Vec::new();
    }
    let params = unsafe { &*params };
    if params.rgvarg.is_null() || params.cArgs == 0 {
        return Vec::new();
    }
    // Automation arguments are stored in reverse order. Reverse again for logs.
    let values = unsafe { std::slice::from_raw_parts(params.rgvarg, params.cArgs as usize) };
    values.iter().rev().filter_map(variant_to_string).collect()
}

fn variant_to_string(value: &VARIANT) -> Option<String> {
    let inner = unsafe { &value.Anonymous.Anonymous };
    match inner.vt {
        VT_I2 => Some(unsafe { inner.Anonymous.iVal }.to_string()),
        VT_I4 | VT_INT => Some(unsafe { inner.Anonymous.lVal }.to_string()),
        VT_UI2 => Some(unsafe { inner.Anonymous.uiVal }.to_string()),
        VT_UI4 | VT_UINT => Some(unsafe { inner.Anonymous.ulVal }.to_string()),
        VT_BSTR => {
            let bstr: &ManuallyDrop<BSTR> = unsafe { &inner.Anonymous.bstrVal };
            Some(bstr.to_string())
        }
        _ => None,
    }
}

windows_core::imp::define_interface!(
    IMsTscNonScriptable,
    IMsTscNonScriptable_Vtbl,
    0xc1e6743a_41c1_4a74_832a_0dd06c1c7a0e
);
windows_core::imp::interface_hierarchy!(IMsTscNonScriptable, IUnknown);

#[repr(C)]
pub struct IMsTscNonScriptable_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    put_clear_text_password:
        unsafe extern "system" fn(*mut c_void, *mut c_void) -> windows_core::HRESULT,
}

impl IMsTscNonScriptable {
    unsafe fn set_clear_text_password(&self, password: &BSTR) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_clear_text_password)(
                Interface::as_raw(self),
                std::mem::transmute_copy(password),
            )
            .ok()
        }
    }
}

#[repr(transparent)]
struct DispatchValue(VARIANT);

impl DispatchValue {
    fn integer(value: i32) -> Self {
        let mut variant = VARIANT::default();
        let inner = unsafe { &mut *variant.Anonymous.Anonymous };
        inner.vt = VT_I4;
        inner.Anonymous.lVal = value;
        Self(variant)
    }

    fn unsigned(value: u32) -> Self {
        let mut variant = VARIANT::default();
        let inner = unsafe { &mut *variant.Anonymous.Anonymous };
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = value;
        Self(variant)
    }

    fn boolean(value: bool) -> Self {
        let mut variant = VARIANT::default();
        let inner = unsafe { &mut *variant.Anonymous.Anonymous };
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = VARIANT_BOOL(if value { -1 } else { 0 });
        Self(variant)
    }

    fn string(value: &str) -> Self {
        let mut variant = VARIANT::default();
        let inner = unsafe { &mut *variant.Anonymous.Anonymous };
        inner.vt = VT_BSTR;
        inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(value));
        Self(variant)
    }
}

impl Drop for DispatchValue {
    fn drop(&mut self) {
        let _ = unsafe { VariantClear(&mut self.0) };
    }
}

trait DispatchExt {
    fn dispatch_id(&self, name: &str) -> windows::core::Result<i32>;
    fn set_property(&self, name: &str, value: &mut DispatchValue) -> windows::core::Result<()>;
    fn get_dispatch(&self, names: &[&str]) -> windows::core::Result<IDispatch>;
    fn invoke_no_args(&self, name: &str) -> windows::core::Result<()>;
    fn invoke_with_arguments(
        &self,
        name: &str,
        arguments: Vec<DispatchValue>,
    ) -> windows::core::Result<()>;
}

impl DispatchExt for IDispatch {
    fn dispatch_id(&self, name: &str) -> windows::core::Result<i32> {
        let wide = name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let name = PCWSTR(wide.as_ptr());
        let mut dispid = 0;
        unsafe {
            self.GetIDsOfNames(&GUID::zeroed(), &name, 1, 0, &mut dispid)?;
        }
        Ok(dispid)
    }

    fn set_property(&self, name: &str, value: &mut DispatchValue) -> windows::core::Result<()> {
        let dispid = self.dispatch_id(name)?;
        let mut named_argument = DISPID_PROPERTYPUT;
        let parameters = DISPPARAMS {
            rgvarg: &mut value.0,
            rgdispidNamedArgs: &mut named_argument,
            cArgs: 1,
            cNamedArgs: 1,
        };
        unsafe {
            self.Invoke(
                dispid,
                &GUID::zeroed(),
                0,
                DISPATCH_PROPERTYPUT,
                &parameters,
                None,
                None,
                None,
            )
        }
    }

    fn get_dispatch(&self, names: &[&str]) -> windows::core::Result<IDispatch> {
        let mut last_error = WinError::from_hresult(E_NOINTERFACE);
        for name in names {
            let attempt = (|| {
                let dispid = self.dispatch_id(name)?;
                let parameters = DISPPARAMS::default();
                let mut result = VARIANT::default();
                unsafe {
                    self.Invoke(
                        dispid,
                        &GUID::zeroed(),
                        0,
                        DISPATCH_PROPERTYGET,
                        &parameters,
                        Some(&mut result),
                        None,
                        None,
                    )?;
                    let inner = &mut *result.Anonymous.Anonymous;
                    if inner.vt != VT_DISPATCH {
                        let _ = VariantClear(&mut result);
                        return Err(WinError::from_hresult(E_NOINTERFACE));
                    }
                    let dispatch = ManuallyDrop::take(&mut inner.Anonymous.pdispVal)
                        .ok_or_else(|| WinError::from_hresult(E_NOINTERFACE))?;
                    inner.vt = VT_EMPTY;
                    Ok(dispatch)
                }
            })();
            match attempt {
                Ok(dispatch) => return Ok(dispatch),
                Err(error) => last_error = error,
            }
        }
        Err(last_error)
    }

    fn invoke_no_args(&self, name: &str) -> windows::core::Result<()> {
        let dispid = self.dispatch_id(name)?;
        let parameters = DISPPARAMS::default();
        unsafe {
            self.Invoke(
                dispid,
                &GUID::zeroed(),
                0,
                DISPATCH_METHOD,
                &parameters,
                None,
                None,
                None,
            )
        }
    }

    fn invoke_with_arguments(
        &self,
        name: &str,
        mut arguments: Vec<DispatchValue>,
    ) -> windows::core::Result<()> {
        let dispid = self.dispatch_id(name)?;
        // IDispatch stores positional arguments right-to-left.
        arguments.reverse();
        let parameters = DISPPARAMS {
            rgvarg: arguments
                .first_mut()
                .map_or(std::ptr::null_mut(), |value| &mut value.0),
            cArgs: arguments.len() as u32,
            ..Default::default()
        };
        unsafe {
            self.Invoke(
                dispid,
                &GUID::zeroed(),
                0,
                DISPATCH_METHOD,
                &parameters,
                None,
                None,
                None,
            )
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionDisplaySettings {
    width: u32,
    height: u32,
    physical_width: u32,
    physical_height: u32,
    desktop_scale_factor: u32,
    device_scale_factor: u32,
}

impl SessionDisplaySettings {
    fn from_window(width: u32, height: u32, dpi: u32, document: &RdpDocument) -> Self {
        let width = width.clamp(200, 8192);
        let height = height.clamp(200, 8192);
        let dpi = dpi.max(96);
        let desktop_scale_factor = document
            .get_integer("desktopscalefactor")
            .filter(|value| (100..=500).contains(value))
            .map(|value| value as u32)
            .unwrap_or_else(|| ((dpi.saturating_mul(100) + 48) / 96).clamp(100, 500));
        let device_scale_factor = document
            .get_integer("devicescalefactor")
            .filter(|value| matches!(value, 100 | 140 | 180))
            .map(|value| value as u32)
            .unwrap_or(100);

        Self {
            width,
            height,
            physical_width: pixels_to_millimeters(width, dpi),
            physical_height: pixels_to_millimeters(height, dpi),
            desktop_scale_factor,
            device_scale_factor,
        }
    }

    fn arguments(self) -> Vec<DispatchValue> {
        vec![
            DispatchValue::unsigned(self.width),
            DispatchValue::unsigned(self.height),
            DispatchValue::unsigned(self.physical_width),
            DispatchValue::unsigned(self.physical_height),
            DispatchValue::unsigned(0), // landscape orientation
            DispatchValue::unsigned(self.desktop_scale_factor),
            DispatchValue::unsigned(self.device_scale_factor),
        ]
    }
}

fn pixels_to_millimeters(pixels: u32, dpi: u32) -> u32 {
    // 1 inch = 25.4 mm. Keep the monitor attributes inside the ranges
    // accepted by the RDP display-control virtual channel.
    (((u64::from(pixels) * 254 + u64::from(dpi) * 5) / (u64::from(dpi) * 10)) as u32)
        .clamp(10, 10_000)
}

pub(super) struct ActiveXHost {
    site: ComObject<ActiveXSite>,
    ole_object: IOleObject,
    inplace_object: Option<IOleInPlaceObject>,
    active_object: Option<IOleInPlaceActiveObject>,
    client: IDispatch,
    event_connection: Option<EventConnection>,
    events: Arc<Mutex<Vec<RdpEvent>>>,
    // Must be dropped after every COM interface sourced from mstscax.dll.
    _rdp_module: LoadedModule,
}

struct EventConnection {
    point: IConnectionPoint,
    cookie: u32,
    _callback: IDispatch,
}

struct LoadedModule(HMODULE);

impl LoadedModule {
    fn load_system_rdp() -> Result<Self> {
        let module = unsafe {
            LoadLibraryExW(
                windows::core::w!("mstscax.dll"),
                None,
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        }
        .windows_context("loading the system RDP control from System32")?;
        Ok(Self(module))
    }
}

impl Drop for LoadedModule {
    fn drop(&mut self) {
        let _ = unsafe { FreeLibrary(self.0) };
    }
}

fn split_server_port(server: &str) -> (&str, Option<u16>) {
    if let Some(bracketed) = server.strip_prefix('[')
        && let Some((host, suffix)) = bracketed.split_once(']')
    {
        let port = suffix
            .strip_prefix(':')
            .and_then(|value| value.parse::<u16>().ok());
        return (host, port);
    }
    if server.bytes().filter(|byte| *byte == b':').count() == 1
        && let Some((host, port)) = server.rsplit_once(':')
        && let Ok(port) = port.parse::<u16>()
    {
        return (host, Some(port));
    }
    (server, None)
}

fn set_optional(dispatch: &IDispatch, property: &str, mut value: DispatchValue) {
    if let Err(error) = dispatch.set_property(property, &mut value) {
        tracing::debug!(%property, %error, "RDP setting is unavailable on this Windows version");
    }
}

fn set_document_bool(dispatch: &IDispatch, property: &str, document: &RdpDocument, key: &str) {
    if let Some(value) = document.get_integer(key) {
        set_optional(dispatch, property, DispatchValue::boolean(value != 0));
    }
}

fn create_desktop_rdp_control() -> Result<IUnknown> {
    let mut last_error = None;
    for clsid in [
        CLSID_MS_RDP_CLIENT_10_NOT_SAFE_FOR_SCRIPTING,
        CLSID_MS_RDP_CLIENT_9_NOT_SAFE_FOR_SCRIPTING,
    ] {
        match unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) } {
            Ok(control) => return Ok(control),
            Err(error) => last_error = Some(error),
        }
    }
    Err(crate::error::Error::Windows {
        context: "creating the desktop Remote Desktop ActiveX control",
        source: last_error.unwrap_or_else(|| WinError::from_hresult(E_NOINTERFACE)),
    })
}

impl ActiveXHost {
    fn activate(hwnd: HWND) -> Result<Self> {
        // Never search the application directory for mstscax.dll. Apart from
        // detecting a damaged Windows installation early, this prevents a DLL
        // placed next to the executable from shadowing the serviced system copy.
        let rdp_module = LoadedModule::load_system_rdp()?;
        let site = ComObject::new(ActiveXSite { hwnd });
        let unknown = create_desktop_rdp_control()?;
        let ole_object: IOleObject = unknown
            .cast()
            .windows_context("querying the RDP control's IOleObject interface")?;
        let client: IDispatch = unknown
            .cast()
            .windows_context("querying the RDP control's automation interface")?;

        let client_site = site.to_interface::<IOleClientSite>();
        unsafe {
            ole_object
                .SetClientSite(&client_site)
                .windows_context("setting the ActiveX client site")?;
            let _ =
                ole_object.SetHostNames(windows::core::w!("mstsc-rs"), windows::core::w!("RDP"));
            if let Ok(persist) = unknown.cast::<IPersistStreamInit>() {
                let _ = persist.InitNew();
            }
            OleSetContainedObject(&unknown, true)
                .windows_context("marking the RDP control as contained")?;
        }

        let mut rect = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rect) }.windows_context("querying the client area")?;
        unsafe {
            if let Err(source) = ole_object.DoVerb(
                OLEIVERB_INPLACEACTIVATE.0,
                std::ptr::null(),
                &client_site,
                0,
                hwnd,
                &rect,
            ) {
                let _ = ole_object.SetClientSite(None);
                return Err(crate::error::Error::Windows {
                    context: "activating the embedded RDP control",
                    source,
                });
            }
        }

        Ok(Self {
            site,
            inplace_object: unknown.cast().ok(),
            active_object: unknown.cast().ok(),
            ole_object,
            client,
            event_connection: None,
            events: Arc::new(Mutex::new(Vec::new())),
            _rdp_module: rdp_module,
        })
    }

    pub fn create(hwnd: HWND, config: &SessionConfig) -> Result<Self> {
        let mut host = Self::activate(hwnd)?;
        host.apply_settings(config)?;
        host.attach_events(hwnd);
        let mut rect = RECT::default();
        unsafe { GetClientRect(hwnd, &mut rect) }
            .windows_context("querying the RDP control area")?;
        host.resize(rect);
        host.client
            .invoke_no_args("Connect")
            .windows_context("starting the RDP connection")?;
        Ok(host)
    }

    fn apply_settings(&self, config: &SessionConfig) -> Result<()> {
        let server = config.server.as_deref().ok_or_else(|| {
            crate::error::Error::CommandLine("server address is required".to_owned())
        })?;
        let username = config
            .username
            .as_deref()
            .ok_or_else(|| crate::error::Error::CommandLine("user name is required".to_owned()))?;
        let password = config.password.as_ref().ok_or_else(|| {
            crate::error::Error::CommandLine(
                "the desktop ActiveX control requires a password supplied by the user".to_owned(),
            )
        })?;
        let (server, port) = split_server_port(server);

        self.client
            .set_property("Server", &mut DispatchValue::string(server))
            .windows_context("setting the RDP server address")?;
        self.client
            .set_property("UserName", &mut DispatchValue::string(username))
            .windows_context("setting the RDP user name")?;
        if let Some(domain) = config.domain.as_deref() {
            self.client
                .set_property("Domain", &mut DispatchValue::string(domain))
                .windows_context("setting the RDP domain")?;
        }

        let width = config
            .document
            .get_integer("desktopwidth")
            .unwrap_or(1280)
            .clamp(200, 7680);
        let height = config
            .document
            .get_integer("desktopheight")
            .unwrap_or(800)
            .clamp(200, 4320);
        let color_depth = config
            .document
            .get_integer("session bpp")
            .unwrap_or(32)
            .clamp(15, 32);
        self.client
            .set_property("DesktopWidth", &mut DispatchValue::integer(width))
            .windows_context("setting the remote desktop width")?;
        self.client
            .set_property("DesktopHeight", &mut DispatchValue::integer(height))
            .windows_context("setting the remote desktop height")?;
        self.client
            .set_property("ColorDepth", &mut DispatchValue::integer(color_depth))
            .windows_context("setting the remote desktop color depth")?;
        self.client
            .set_property("FullScreen", &mut DispatchValue::boolean(config.fullscreen))
            .windows_context("setting full-screen mode")?;

        let non_scriptable: IMsTscNonScriptable = self
            .client
            .cast()
            .windows_context("opening the secure RDP credential interface")?;
        let password = BSTR::from(password.expose());
        unsafe { non_scriptable.set_clear_text_password(&password) }
            .windows_context("passing the password to the system RDP control")?;

        let advanced = self
            .client
            .get_dispatch(&[
                "AdvancedSettings9",
                "AdvancedSettings8",
                "AdvancedSettings7",
                "AdvancedSettings6",
                "AdvancedSettings5",
                "AdvancedSettings4",
                "AdvancedSettings3",
                "AdvancedSettings2",
                "AdvancedSettings",
            ])
            .windows_context("opening the desktop RDP advanced settings")?;

        if let Some(port) = port {
            set_optional(
                &advanced,
                "RDPPort",
                DispatchValue::integer(i32::from(port)),
            );
        }
        set_document_bool(
            &advanced,
            "RedirectPrinters",
            &config.document,
            "redirectprinters",
        );
        set_document_bool(
            &advanced,
            "RedirectSmartCards",
            &config.document,
            "redirectsmartcards",
        );
        set_document_bool(
            &advanced,
            "RedirectPorts",
            &config.document,
            "redirectcomports",
        );
        set_document_bool(
            &advanced,
            "RedirectClipboard",
            &config.document,
            "redirectclipboard",
        );
        set_document_bool(
            &advanced,
            "AudioCaptureRedirectionMode",
            &config.document,
            "audiocapturemode",
        );
        set_document_bool(
            &advanced,
            "EnableCredSspSupport",
            &config.document,
            "enablecredsspsupport",
        );
        set_document_bool(
            &advanced,
            "ConnectToAdministerServer",
            &config.document,
            "administrative session",
        );
        set_document_bool(&advanced, "PublicMode", &config.document, "public mode");
        // SmartSizing is the visual fallback for servers that do not support
        // live display updates. It also keeps the old framebuffer fitted while
        // a resize update is in flight.
        set_optional(
            &advanced,
            "SmartSizing",
            DispatchValue::boolean(config.dynamic_resolution),
        );
        for (property, key) in [
            ("AudioRedirectionMode", "audiomode"),
            ("AuthenticationLevel", "authentication level"),
            ("PerformanceFlags", "performance flags"),
        ] {
            if let Some(value) = config.document.get_integer(key) {
                set_optional(&advanced, property, DispatchValue::integer(value));
            }
        }
        if let Some(load_balance_info) = config.document.get_string("loadbalanceinfo") {
            set_optional(
                &advanced,
                "LoadBalanceInfo",
                DispatchValue::string(load_balance_info),
            );
        }
        if let Some(drives) = config.document.get_string("drivestoredirect") {
            set_optional(
                &advanced,
                "RedirectDrives",
                DispatchValue::boolean(!drives.is_empty()),
            );
        }

        self.apply_gateway_settings(config);
        self.apply_remote_app_settings(config);
        Ok(())
    }

    fn apply_gateway_settings(&self, config: &SessionConfig) {
        let Some(hostname) = config.document.get_string("gatewayhostname") else {
            return;
        };
        let Ok(transport) = self.client.get_dispatch(&[
            "TransportSettings3",
            "TransportSettings2",
            "TransportSettings",
        ]) else {
            tracing::warn!("the installed RDP control does not expose RD Gateway settings");
            return;
        };
        set_optional(
            &transport,
            "GatewayHostname",
            DispatchValue::string(hostname),
        );
        for (property, key) in [
            ("GatewayUsageMethod", "gatewayusagemethod"),
            ("GatewayProfileUsageMethod", "gatewayprofileusagemethod"),
            ("GatewayCredsSource", "gatewaycredentialssource"),
        ] {
            if let Some(value) = config.document.get_integer(key) {
                set_optional(&transport, property, DispatchValue::integer(value));
            }
        }
    }

    fn apply_remote_app_settings(&self, config: &SessionConfig) {
        if config.document.get_integer("remoteapplicationmode") != Some(1) {
            return;
        }
        let Ok(remote_program) = self
            .client
            .get_dispatch(&["RemoteProgram2", "RemoteProgram"])
        else {
            tracing::warn!("the installed RDP control does not expose RemoteApp settings");
            return;
        };
        set_optional(
            &remote_program,
            "RemoteProgramMode",
            DispatchValue::boolean(true),
        );
        if let Some(program) = config.document.get_string("remoteapplicationprogram") {
            set_optional(
                &remote_program,
                "RemoteApplicationProgram",
                DispatchValue::string(program),
            );
        }
        if let Some(arguments) = config.document.get_string("remoteapplicationcmdline") {
            set_optional(
                &remote_program,
                "RemoteApplicationArgs",
                DispatchValue::string(arguments),
            );
        }
    }

    fn attach_events(&mut self, hwnd: HWND) {
        let result = (|| {
            let container: IConnectionPointContainer = self.client.cast()?;
            let point = unsafe { container.FindConnectionPoint(&DIID_MS_TSC_AX_EVENTS) }?;
            let callback: IDispatch = ComObject::new(EventCallback {
                hwnd,
                events: self.events.clone(),
            })
            .into_interface();
            let cookie = unsafe { point.Advise(&callback) }?;
            Ok::<_, WinError>(EventConnection {
                point,
                cookie,
                _callback: callback,
            })
        })();
        match result {
            Ok(connection) => self.event_connection = Some(connection),
            Err(error) => tracing::debug!(%error, "RDP event connection is unavailable"),
        }
    }

    pub fn resize(&self, rect: RECT) {
        if let Some(inplace) = &self.inplace_object {
            let _ = unsafe { inplace.SetObjectRects(&rect, &rect) };
        }
    }

    pub fn update_display(&self, width: u32, height: u32, dpi: u32, document: &RdpDocument) {
        if width == 0 || height == 0 {
            return;
        }
        let settings = SessionDisplaySettings::from_window(width, height, dpi, document);
        if let Err(error) = self
            .client
            .invoke_with_arguments("UpdateSessionDisplaySettings", settings.arguments())
        {
            // SmartSizing remains enabled, so older servers and controls still
            // resize cleanly without exposing scroll bars or clipped content.
            tracing::debug!(%error, "live RDP display update is unavailable; using SmartSizing");
        }
    }

    pub fn translate_accelerator(&self, message: &MSG) -> bool {
        self.active_object.as_ref().is_some_and(|active| {
            // The generated wrapper turns both S_OK and S_FALSE into Ok(()),
            // but OLE defines only S_OK as "message handled". Calling the
            // vtable directly preserves S_FALSE so ordinary window, paint and
            // posted RDP-event messages continue through DispatchMessageW.
            let result = unsafe {
                (Interface::vtable(active).TranslateAccelerator)(Interface::as_raw(active), message)
            };
            accelerator_was_handled(result)
        })
    }

    pub fn disconnect(&self) {
        let _ = self.client.invoke_no_args("Disconnect");
    }

    pub fn take_events(&self) -> Vec<RdpEvent> {
        self.events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }
}

fn accelerator_was_handled(result: HRESULT) -> bool {
    result == S_OK
}

impl Drop for ActiveXHost {
    fn drop(&mut self) {
        if let Some(connection) = self.event_connection.take() {
            let _ = unsafe { connection.point.Unadvise(connection.cookie) };
        }
        let _ = self.client.invoke_no_args("Disconnect");
        if let Some(inplace) = &self.inplace_object {
            let _ = unsafe { inplace.UIDeactivate() };
            let _ = unsafe { inplace.InPlaceDeactivate() };
        }
        let _ = unsafe { self.ole_object.Close(OLECLOSE_NOSAVE) };
        let _ = unsafe { self.ole_object.SetClientSite(None) };
        // Keep the site alive through SetClientSite(None).
        let _ = &self.site;
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{E_FAIL, HWND, S_FALSE, S_OK};
    use windows::Win32::System::Ole::{OleInitialize, OleUninitialize};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_OVERLAPPED, WS_VISIBLE,
    };
    use windows::core::w;

    use crate::config::{ConnectionOverrides, SecretString, SessionConfig};
    use crate::rdp::RdpDocument;

    use super::{ActiveXHost, DispatchExt, SessionDisplaySettings, accelerator_was_handled};

    struct OleGuard;

    impl Drop for OleGuard {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }

    struct WindowGuard(HWND);

    impl Drop for WindowGuard {
        fn drop(&mut self) {
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn accelerator_only_consumes_messages_reported_as_handled() {
        assert!(accelerator_was_handled(S_OK));
        assert!(!accelerator_was_handled(S_FALSE));
        assert!(!accelerator_was_handled(E_FAIL));
    }

    #[test]
    fn display_settings_follow_window_size_and_dpi() {
        let settings = SessionDisplaySettings::from_window(50, 9_000, 144, &RdpDocument::default());
        assert_eq!(settings.width, 200);
        assert_eq!(settings.height, 8_192);
        assert_eq!(settings.desktop_scale_factor, 150);
        assert_eq!(settings.device_scale_factor, 100);
        assert!((10..=10_000).contains(&settings.physical_width));
        assert!((10..=10_000).contains(&settings.physical_height));

        let document =
            RdpDocument::parse("desktopscalefactor:i:175\r\ndevicescalefactor:i:140\r\n");
        let configured = SessionDisplaySettings::from_window(1_280, 720, 96, &document);
        assert_eq!(configured.desktop_scale_factor, 175);
        assert_eq!(configured.device_scale_factor, 140);
    }

    #[test]
    fn system_rdp_control_can_be_activated_and_configured() {
        // Use a fresh thread so no other test can have selected a conflicting
        // COM apartment model. This is a runtime test, not merely a registry
        // or type check: it executes the same OLE activation as the application.
        std::thread::spawn(|| {
            unsafe { OleInitialize(None) }.expect("OLE STA initialization must succeed");
            let _ole = OleGuard;

            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("mstsc-rs COM activation test"),
                    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_VISIBLE.0),
                    0,
                    0,
                    640,
                    480,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("test host window must be created");
            let window = WindowGuard(hwnd);

            let host = ActiveXHost::activate(window.0)
                .expect("the system Remote Desktop ActiveX control must activate");
            host.client
                .dispatch_id("UpdateSessionDisplaySettings")
                .expect("the version 9/10 RDP control must expose live display updates");
            let config = SessionConfig::resolve(
                None,
                ConnectionOverrides {
                    server: Some("localhost:3389".to_owned()),
                    username: Some("mstsc-rs-ci".to_owned()),
                    password: Some(SecretString::new("not-a-real-password")),
                    ..Default::default()
                },
            )
            .expect("test RDP settings must resolve");
            host.apply_settings(&config)
                .expect("the desktop RDP control must accept core settings and credentials");
            drop(host);
            drop(window);
        })
        .join()
        .expect("COM activation test thread must not panic");
    }
}
