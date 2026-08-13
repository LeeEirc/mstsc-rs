use std::ffi::c_void;
use std::mem::{ManuallyDrop, size_of};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_LOCATE_DEVNODE_NORMAL, CM_Locate_DevNodeW, CR_SUCCESS,
};
use windows::Win32::Devices::Properties::{DEVPKEY_Device_ClassGuid, DEVPROP_TYPE_GUID};
use windows::Win32::Foundation::{
    DISP_E_UNKNOWNNAME, E_NOINTERFACE, E_NOTIMPL, E_POINTER, FreeLibrary, HMODULE, HWND, LPARAM,
    RECT, S_OK, SIZE, VARIANT_BOOL, WPARAM,
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
use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
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

// The connection point does not accept a sink that only identifies itself as
// IDispatch. Its QueryInterface must also recognize the outgoing dispinterface
// IID, even though that interface uses the ordinary IDispatch vtable.
windows_core::imp::define_interface!(
    IMsTscAxEvents,
    IMsTscAxEvents_Vtbl,
    0x336d5562_efa8_482e_8cb3_c5c0fc7a7db6
);

impl std::ops::Deref for IMsTscAxEvents {
    type Target = IDispatch;

    fn deref(&self) -> &Self::Target {
        unsafe { std::mem::transmute(self) }
    }
}

windows_core::imp::interface_hierarchy!(IMsTscAxEvents, IUnknown, IDispatch);

impl windows_core::RuntimeName for IMsTscAxEvents {}

#[repr(C)]
pub struct IMsTscAxEvents_Vtbl {
    base__: windows::Win32::System::Com::IDispatch_Vtbl,
}

#[allow(non_camel_case_types)]
pub trait IMsTscAxEvents_Impl: IDispatch_Impl {}

impl IMsTscAxEvents_Vtbl {
    pub const fn new<Identity: IMsTscAxEvents_Impl, const OFFSET: isize>() -> Self {
        Self {
            base__: windows::Win32::System::Com::IDispatch_Vtbl::new::<Identity, OFFSET>(),
        }
    }

    pub fn matches(iid: &GUID) -> bool {
        iid == &IMsTscAxEvents::IID || iid == &IDispatch::IID
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RdpEventKind {
    Connecting,
    Connected,
    LoginCompleted,
    Disconnected,
    AutoReconnecting,
    AutoReconnected,
    DialogDisplaying,
    DialogDismissed,
    EnterFullScreen,
    LeaveFullScreen,
    RequestGoFullScreen,
    RequestLeaveFullScreen,
    RequestContainerMinimize,
    FatalError,
    Warning,
    LogonError,
    NetworkStatusChanged,
    RemoteDesktopSizeChanged,
    RemoteProgramDisplayed,
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

#[windows::core::implement(IMsTscAxEvents)]
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

impl IMsTscAxEvents_Impl for EventCallback_Impl {}

fn legacy_event_kind(dispid: i32) -> Option<RdpEventKind> {
    match dispid {
        1 => Some(RdpEventKind::Connecting),
        2 => Some(RdpEventKind::Connected),
        3 => Some(RdpEventKind::LoginCompleted),
        4 => Some(RdpEventKind::Disconnected),
        5 => Some(RdpEventKind::EnterFullScreen),
        6 => Some(RdpEventKind::LeaveFullScreen),
        8 => Some(RdpEventKind::RequestGoFullScreen),
        9 => Some(RdpEventKind::RequestLeaveFullScreen),
        10 => Some(RdpEventKind::FatalError),
        11 => Some(RdpEventKind::Warning),
        12 => Some(RdpEventKind::RemoteDesktopSizeChanged),
        14 => Some(RdpEventKind::RequestContainerMinimize),
        17 | 34 => Some(RdpEventKind::AutoReconnecting),
        18 => Some(RdpEventKind::DialogDisplaying),
        19 => Some(RdpEventKind::DialogDismissed),
        21 | 29 => Some(RdpEventKind::RemoteProgramDisplayed),
        22 => Some(RdpEventKind::LogonError),
        32 => Some(RdpEventKind::NetworkStatusChanged),
        33 => Some(RdpEventKind::AutoReconnected),
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

windows_core::imp::define_interface!(
    IMsRdpExtendedSettings,
    IMsRdpExtendedSettings_Vtbl,
    0x302d8188_0052_4807_806a_362b628f9ac5
);
windows_core::imp::interface_hierarchy!(IMsRdpExtendedSettings, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpClientNonScriptable5,
    IMsRdpClientNonScriptable5_Vtbl,
    0x4f6996d5_d7b1_412c_b0ff_063718566907
);
windows_core::imp::interface_hierarchy!(IMsRdpClientNonScriptable5, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpClientNonScriptable,
    IMsRdpClientNonScriptable_Vtbl,
    0x2f079c4c_87b2_4afd_97ab_20cdb43038ae
);
windows_core::imp::interface_hierarchy!(IMsRdpClientNonScriptable, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpClientNonScriptable3,
    IMsRdpClientNonScriptable3_Vtbl,
    0xb3378d90_0728_45c7_8ed7_b6159fb92219
);
windows_core::imp::interface_hierarchy!(IMsRdpClientNonScriptable3, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpClientNonScriptable7,
    IMsRdpClientNonScriptable7_Vtbl,
    0x71b4a60a_fe21_46d8_a39b_8e32ba0c5ecc
);
windows_core::imp::interface_hierarchy!(IMsRdpClientNonScriptable7, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpDeviceCollection,
    IMsRdpDeviceCollection_Vtbl,
    0x56540617_d281_488c_8738_6a8fdf64a118
);
windows_core::imp::interface_hierarchy!(IMsRdpDeviceCollection, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpDevice,
    IMsRdpDevice_Vtbl,
    0x60c3b9c8_9e92_4f5e_a3e7_604a912093ea
);
windows_core::imp::interface_hierarchy!(IMsRdpDevice, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpDeviceCollection2,
    IMsRdpDeviceCollection2_Vtbl,
    0xe0e5d68a_f2e7_4350_adfe_ac0e08d74de0
);
windows_core::imp::interface_hierarchy!(IMsRdpDeviceCollection2, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpDeviceV2,
    IMsRdpDeviceV2_Vtbl,
    0x5fb94466_7661_42a8_98b7_01904c11668f
);
windows_core::imp::interface_hierarchy!(IMsRdpDeviceV2, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpDriveCollection,
    IMsRdpDriveCollection_Vtbl,
    0x7ff17599_da2c_4677_ad35_f60c04fe1585
);
windows_core::imp::interface_hierarchy!(IMsRdpDriveCollection, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpDrive,
    IMsRdpDrive_Vtbl,
    0xd28b5458_f694_47a8_8e61_40356a767e46
);
windows_core::imp::interface_hierarchy!(IMsRdpDrive, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpCameraRedirConfigCollection,
    IMsRdpCameraRedirConfigCollection_Vtbl,
    0xae45252b_aaab_4504_b681_649d6073a37a
);
windows_core::imp::interface_hierarchy!(IMsRdpCameraRedirConfigCollection, IUnknown);

windows_core::imp::define_interface!(
    IMsRdpCameraRedirConfig,
    IMsRdpCameraRedirConfig_Vtbl,
    0x09750604_d625_47c1_9fcd_f09f735705d7
);
windows_core::imp::interface_hierarchy!(IMsRdpCameraRedirConfig, IUnknown);

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

#[repr(C)]
pub struct IMsRdpExtendedSettings_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    put_property:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut VARIANT) -> windows_core::HRESULT,
    get_property:
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut VARIANT) -> windows_core::HRESULT,
}

impl IMsRdpExtendedSettings {
    unsafe fn set_named_property(
        &self,
        name: &BSTR,
        value: *mut VARIANT,
    ) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_property)(
                Interface::as_raw(self),
                std::mem::transmute_copy(name),
                value,
            )
            .ok()
        }
    }
}

#[repr(C)]
pub struct IMsRdpClientNonScriptable5_Vtbl {
    // The version 5 interface inherits the earlier non-scriptable interfaces.
    // The registered type library fixes put_UseMultimon at byte offset 424 on
    // x64 (vtable slot 53, including IUnknown's three slots).
    inherited_slots: [usize; 53],
    put_use_multimon: unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
}

impl IMsRdpClientNonScriptable5 {
    unsafe fn set_use_multimon(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_use_multimon)(
                Interface::as_raw(self),
                VARIANT_BOOL(if enabled { -1 } else { 0 }),
            )
            .ok()
        }
    }
}

#[repr(C)]
pub struct IMsRdpClientNonScriptable_Vtbl {
    // IMsTscNonScriptable occupies slots 0 through 12. The registered type
    // library fixes NotifyRedirectDeviceChange at x64 byte offset 104.
    inherited_slots: [usize; 13],
    notify_redirect_device_change:
        unsafe extern "system" fn(*mut c_void, usize, isize) -> windows_core::HRESULT,
}

impl IMsRdpClientNonScriptable {
    unsafe fn notify_redirect_device_change(
        &self,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).notify_redirect_device_change)(
                Interface::as_raw(self),
                wparam.0,
                lparam.0,
            )
            .ok()
        }
    }
}

#[repr(C)]
pub struct IMsRdpClientNonScriptable3_Vtbl {
    // The registered type library fixes these methods at x64 byte offsets
    // 200 through 240.
    inherited_slots: [usize; 25],
    put_redirect_dynamic_drives:
        unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_redirect_dynamic_drives: usize,
    put_redirect_dynamic_devices:
        unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_redirect_dynamic_devices: usize,
    get_device_collection:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> windows_core::HRESULT,
    get_drive_collection:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> windows_core::HRESULT,
}

impl IMsRdpClientNonScriptable3 {
    unsafe fn set_redirect_dynamic_drives(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_redirect_dynamic_drives)(
                Interface::as_raw(self),
                variant_bool(enabled),
            )
            .ok()
        }
    }

    unsafe fn set_redirect_dynamic_devices(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_redirect_dynamic_devices)(
                Interface::as_raw(self),
                variant_bool(enabled),
            )
            .ok()
        }
    }

    unsafe fn device_collection(&self) -> windows::core::Result<IMsRdpDeviceCollection> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            (Interface::vtable(self).get_device_collection)(Interface::as_raw(self), &mut raw)
                .ok()?;
            interface_from_raw(raw)
        }
    }

    unsafe fn drive_collection(&self) -> windows::core::Result<IMsRdpDriveCollection> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            (Interface::vtable(self).get_drive_collection)(Interface::as_raw(self), &mut raw)
                .ok()?;
            interface_from_raw(raw)
        }
    }
}

#[repr(C)]
pub struct IMsRdpClientNonScriptable7_Vtbl {
    // CameraRedirConfigCollection is at x64 byte offset 536 (slot 67).
    inherited_slots: [usize; 67],
    get_camera_redir_config_collection:
        unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> windows_core::HRESULT,
}

impl IMsRdpClientNonScriptable7 {
    unsafe fn camera_collection(&self) -> windows::core::Result<IMsRdpCameraRedirConfigCollection> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            (Interface::vtable(self).get_camera_redir_config_collection)(
                Interface::as_raw(self),
                &mut raw,
            )
            .ok()?;
            interface_from_raw(raw)
        }
    }
}

#[repr(C)]
pub struct IMsRdpDeviceCollection_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    rescan_devices: unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_device_by_index:
        unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> windows_core::HRESULT,
    get_device_by_id: usize,
    get_device_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> windows_core::HRESULT,
}

impl IMsRdpDeviceCollection {
    unsafe fn rescan(&self, redirect_new: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).rescan_devices)(
                Interface::as_raw(self),
                variant_bool(redirect_new),
            )
            .ok()
        }
    }

    unsafe fn count(&self) -> windows::core::Result<u32> {
        let mut count = 0;
        unsafe {
            (Interface::vtable(self).get_device_count)(Interface::as_raw(self), &mut count).ok()?;
        }
        Ok(count)
    }

    unsafe fn by_index(&self, index: u32) -> windows::core::Result<IMsRdpDevice> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            (Interface::vtable(self).get_device_by_index)(Interface::as_raw(self), index, &mut raw)
                .ok()?;
            interface_from_raw(raw)
        }
    }
}

#[repr(C)]
pub struct IMsRdpDeviceCollection2_Vtbl {
    // IMsRdpDeviceCollection occupies slots 0 through 6 and
    // AddDeviceByInstanceId occupies slot 7.
    inherited_slots: [usize; 8],
    redirect_now: unsafe extern "system" fn(*mut c_void) -> windows_core::HRESULT,
}

impl IMsRdpDeviceCollection2 {
    unsafe fn redirect_now(&self) -> windows::core::Result<()> {
        unsafe { (Interface::vtable(self).redirect_now)(Interface::as_raw(self)).ok() }
    }
}

#[repr(C)]
pub struct IMsRdpDevice_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    get_device_instance_id:
        unsafe extern "system" fn(*mut c_void, *mut BSTR) -> windows_core::HRESULT,
    get_friendly_name: usize,
    get_device_description: usize,
    put_redirection_state:
        unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_redirection_state: usize,
}

impl IMsRdpDevice {
    unsafe fn instance_id(&self) -> windows::core::Result<String> {
        let mut value = BSTR::new();
        unsafe {
            (Interface::vtable(self).get_device_instance_id)(Interface::as_raw(self), &mut value)
                .ok()?;
        }
        Ok(value.to_string())
    }

    unsafe fn set_redirected(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_redirection_state)(
                Interface::as_raw(self),
                variant_bool(enabled),
            )
            .ok()
        }
    }
}

#[repr(C)]
pub struct IMsRdpDeviceV2_Vtbl {
    // IMsRdpDevice occupies slots 0 through 7 and DeviceText occupies slot 8.
    // IsUSBDevice is therefore at x64 byte offset 72 (slot 9).
    inherited_slots: [usize; 9],
    get_is_usb_device:
        unsafe extern "system" fn(*mut c_void, *mut VARIANT_BOOL) -> windows_core::HRESULT,
}

impl IMsRdpDeviceV2 {
    unsafe fn is_usb_device(&self) -> windows::core::Result<bool> {
        let mut value = VARIANT_BOOL(0);
        unsafe {
            (Interface::vtable(self).get_is_usb_device)(Interface::as_raw(self), &mut value)
                .ok()?;
        }
        Ok(value.0 != 0)
    }
}

#[repr(C)]
pub struct IMsRdpDriveCollection_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    rescan_drives: unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_drive_by_index:
        unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> windows_core::HRESULT,
    get_drive_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> windows_core::HRESULT,
}

impl IMsRdpDriveCollection {
    unsafe fn rescan(&self, redirect_new: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).rescan_drives)(
                Interface::as_raw(self),
                variant_bool(redirect_new),
            )
            .ok()
        }
    }

    unsafe fn count(&self) -> windows::core::Result<u32> {
        let mut count = 0;
        unsafe {
            (Interface::vtable(self).get_drive_count)(Interface::as_raw(self), &mut count).ok()?;
        }
        Ok(count)
    }

    unsafe fn by_index(&self, index: u32) -> windows::core::Result<IMsRdpDrive> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            (Interface::vtable(self).get_drive_by_index)(Interface::as_raw(self), index, &mut raw)
                .ok()?;
            interface_from_raw(raw)
        }
    }
}

#[repr(C)]
pub struct IMsRdpDrive_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    get_name: unsafe extern "system" fn(*mut c_void, *mut BSTR) -> windows_core::HRESULT,
    put_redirection_state:
        unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_redirection_state: usize,
}

impl IMsRdpDrive {
    unsafe fn name(&self) -> windows::core::Result<String> {
        let mut value = BSTR::new();
        unsafe {
            (Interface::vtable(self).get_name)(Interface::as_raw(self), &mut value).ok()?;
        }
        Ok(value.to_string())
    }

    unsafe fn set_redirected(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_redirection_state)(
                Interface::as_raw(self),
                variant_bool(enabled),
            )
            .ok()
        }
    }
}

#[repr(C)]
pub struct IMsRdpCameraRedirConfigCollection_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    rescan: unsafe extern "system" fn(*mut c_void) -> windows_core::HRESULT,
    get_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> windows_core::HRESULT,
    get_by_index:
        unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> windows_core::HRESULT,
    get_by_symbolic_link: usize,
    get_by_instance_id: usize,
    add_config: usize,
    put_redirect_by_default:
        unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_redirect_by_default: usize,
    put_encode_video: unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_encode_video: usize,
    put_encoding_quality: unsafe extern "system" fn(*mut c_void, i32) -> windows_core::HRESULT,
    get_encoding_quality: usize,
}

impl IMsRdpCameraRedirConfigCollection {
    unsafe fn rescan(&self) -> windows::core::Result<()> {
        unsafe { (Interface::vtable(self).rescan)(Interface::as_raw(self)).ok() }
    }

    unsafe fn count(&self) -> windows::core::Result<u32> {
        let mut count = 0;
        unsafe {
            (Interface::vtable(self).get_count)(Interface::as_raw(self), &mut count).ok()?;
        }
        Ok(count)
    }

    unsafe fn by_index(&self, index: u32) -> windows::core::Result<IMsRdpCameraRedirConfig> {
        let mut raw = std::ptr::null_mut();
        unsafe {
            (Interface::vtable(self).get_by_index)(Interface::as_raw(self), index, &mut raw)
                .ok()?;
            interface_from_raw(raw)
        }
    }

    unsafe fn set_redirect_by_default(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_redirect_by_default)(
                Interface::as_raw(self),
                variant_bool(enabled),
            )
            .ok()
        }
    }

    unsafe fn set_encode_video(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_encode_video)(
                Interface::as_raw(self),
                variant_bool(enabled),
            )
            .ok()
        }
    }

    unsafe fn set_encoding_quality(&self, quality: i32) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_encoding_quality)(Interface::as_raw(self), quality).ok()
        }
    }
}

#[repr(C)]
pub struct IMsRdpCameraRedirConfig_Vtbl {
    base__: windows_core::IUnknown_Vtbl,
    get_friendly_name: usize,
    get_symbolic_link: unsafe extern "system" fn(*mut c_void, *mut BSTR) -> windows_core::HRESULT,
    get_instance_id: usize,
    get_parent_instance_id: usize,
    put_redirected: unsafe extern "system" fn(*mut c_void, VARIANT_BOOL) -> windows_core::HRESULT,
    get_redirected: usize,
    get_device_exists: usize,
}

impl IMsRdpCameraRedirConfig {
    unsafe fn symbolic_link(&self) -> windows::core::Result<String> {
        let mut value = BSTR::new();
        unsafe {
            (Interface::vtable(self).get_symbolic_link)(Interface::as_raw(self), &mut value)
                .ok()?;
        }
        Ok(value.to_string())
    }

    unsafe fn set_redirected(&self, enabled: bool) -> windows::core::Result<()> {
        unsafe {
            (Interface::vtable(self).put_redirected)(Interface::as_raw(self), variant_bool(enabled))
                .ok()
        }
    }
}

fn variant_bool(value: bool) -> VARIANT_BOOL {
    VARIANT_BOOL(if value { -1 } else { 0 })
}

unsafe fn interface_from_raw<T: Interface>(raw: *mut c_void) -> windows::core::Result<T> {
    let raw = NonNull::new(raw).ok_or_else(|| WinError::from_hresult(E_POINTER))?;
    Ok(unsafe { T::from_raw(raw.as_ptr()) })
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RedirectionSettings {
    drives: Option<String>,
    devices: Option<String>,
    usb_devices: Option<String>,
    cameras: Option<String>,
    encode_video: Option<bool>,
    encoding_quality: Option<i32>,
}

impl RedirectionSettings {
    fn from_document(document: &RdpDocument) -> Self {
        Self {
            drives: document.get_string("drivestoredirect").map(str::to_owned),
            devices: document.get_string("devicestoredirect").map(str::to_owned),
            usb_devices: document
                .get_string("usbdevicestoredirect")
                .map(str::to_owned),
            cameras: document.get_string("camerastoredirect").map(str::to_owned),
            encode_video: document
                .get_integer("encode redirected video capture")
                .map(|value| value != 0),
            encoding_quality: document
                .get_integer("redirected video capture encoding quality")
                .filter(|value| (0..=2).contains(value)),
        }
    }

    fn has_device_configuration(&self) -> bool {
        self.devices.is_some() || self.usb_devices.is_some()
    }

    fn has_camera_configuration(&self) -> bool {
        self.cameras.is_some() || self.encode_video.is_some() || self.encoding_quality.is_some()
    }

    fn redirect_dynamic_drives(&self) -> bool {
        self.drives
            .as_deref()
            .is_some_and(|value| selectors_redirect_new(value, "dynamicdrives"))
    }

    fn redirect_dynamic_devices(&self) -> bool {
        self.devices
            .as_deref()
            .is_some_and(|value| selectors_redirect_new(value, "dynamicdevices"))
            || self
                .usb_devices
                .as_deref()
                .is_some_and(|value| selectors(value).any(|selector| selector == "*"))
    }

    fn drive_is_selected(&self, drive_name: &str) -> Option<bool> {
        let configured = self.drives.as_deref()?;
        let wanted = normalize_drive_name(drive_name);
        Some(selectors(configured).any(|selector| {
            selector == "*"
                || (selector != "dynamicdrives"
                    && normalize_drive_name(selector).eq_ignore_ascii_case(wanted))
        }))
    }

    fn device_is_selected(
        &self,
        instance_id: &str,
        is_usb: bool,
        class_guid: Option<GUID>,
    ) -> Option<bool> {
        let configured = if is_usb {
            self.usb_devices.as_deref()?
        } else {
            self.devices.as_deref()?
        };
        Some(selector_list_matches(
            configured,
            instance_id,
            class_guid,
            if is_usb {
                "dynamicusbdevices"
            } else {
                "dynamicdevices"
            },
        ))
    }

    fn camera_is_selected(&self, symbolic_link: &str) -> Option<bool> {
        let configured = self.cameras.as_deref()?;
        Some(selector_list_matches(configured, symbolic_link, None, ""))
    }
}

fn selectors(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(';')
        .map(str::trim)
        .filter(|selector| !selector.is_empty())
}

fn selectors_redirect_new(value: &str, dynamic_marker: &str) -> bool {
    selectors(value)
        .any(|selector| selector == "*" || selector.eq_ignore_ascii_case(dynamic_marker))
}

fn normalize_drive_name(value: &str) -> &str {
    value.trim().trim_end_matches(['\\', '/'])
}

fn selector_list_matches(
    configured: &str,
    instance_id: &str,
    class_guid: Option<GUID>,
    dynamic_marker: &str,
) -> bool {
    let mut selected = false;
    for selector in selectors(configured).filter(|selector| !selector.starts_with('-')) {
        let is_dynamic_marker =
            !dynamic_marker.is_empty() && selector.eq_ignore_ascii_case(dynamic_marker);
        if selector == "*"
            || (!is_dynamic_marker && selector_matches(selector, instance_id, class_guid))
        {
            selected = true;
        }
    }
    for selector in selectors(configured).filter_map(|selector| selector.strip_prefix('-')) {
        if selector_matches(selector, instance_id, class_guid) {
            selected = false;
        }
    }
    selected
}

fn selector_matches(selector: &str, instance_id: &str, class_guid: Option<GUID>) -> bool {
    if selector == "*" {
        return true;
    }
    if let Some(selector_guid) = selector
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .and_then(|value| GUID::try_from(value).ok())
    {
        return class_guid == Some(selector_guid);
    }
    instance_id
        .get(..selector.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(selector))
}

fn device_class_guid(instance_id: &str) -> Option<GUID> {
    let wide = instance_id
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut device_instance = 0;
    if unsafe {
        CM_Locate_DevNodeW(
            &mut device_instance,
            PCWSTR(wide.as_ptr()),
            CM_LOCATE_DEVNODE_NORMAL,
        )
    } != CR_SUCCESS
    {
        return None;
    }
    let mut class_guid = GUID::zeroed();
    let mut property_type = Default::default();
    let mut buffer_size = size_of::<GUID>() as u32;
    let result = unsafe {
        CM_Get_DevNode_PropertyW(
            device_instance,
            &DEVPKEY_Device_ClassGuid,
            &mut property_type,
            Some((&mut class_guid as *mut GUID).cast()),
            &mut buffer_size,
            0,
        )
    };
    (result == CR_SUCCESS && property_type == DEVPROP_TYPE_GUID).then_some(class_guid)
}

pub(super) struct ActiveXHost {
    site: ComObject<ActiveXSite>,
    ole_object: IOleObject,
    inplace_object: Option<IOleInPlaceObject>,
    active_object: Option<IOleInPlaceActiveObject>,
    client: IDispatch,
    event_connection: Option<EventConnection>,
    events: Arc<Mutex<Vec<RdpEvent>>>,
    redirection: RedirectionSettings,
    // Must be dropped after every COM interface sourced from mstscax.dll.
    _rdp_module: LoadedModule,
}

struct EventConnection {
    point: IConnectionPoint,
    cookie: u32,
    _callback: IMsTscAxEvents,
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

fn set_extended_optional(
    extended: &IMsRdpExtendedSettings,
    property: &str,
    mut value: DispatchValue,
) {
    let name = BSTR::from(property);
    if let Err(error) = unsafe { extended.set_named_property(&name, &mut value.0) } {
        tracing::debug!(%property, %error, "extended RDP setting is unavailable");
    }
}

fn system_webauthn_plugin_path() -> Option<String> {
    let mut buffer = vec![0u16; 512];
    loop {
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if length == 0 || length > 32_767 {
            return None;
        }
        if length < buffer.len() {
            let mut path = String::from_utf16(&buffer[..length]).ok()?;
            path.push_str("\\webauthn.dll");
            return std::path::Path::new(&path).is_file().then_some(path);
        }
        buffer.resize(length + 1, 0);
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
            redirection: RedirectionSettings::default(),
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

    fn apply_settings(&mut self, config: &SessionConfig) -> Result<()> {
        self.redirection = RedirectionSettings::from_document(&config.document);
        let server = config.server.as_deref().ok_or_else(|| {
            crate::error::Error::CommandLine("server address is required".to_owned())
        })?;
        let (server, port) = split_server_port(server);

        self.client
            .set_property("Server", &mut DispatchValue::string(server))
            .windows_context("setting the RDP server address")?;
        if let Some(username) = config.username.as_deref() {
            self.client
                .set_property("UserName", &mut DispatchValue::string(username))
                .windows_context("setting the RDP user name")?;
        }
        if let Some(domain) = config.domain.as_deref() {
            self.client
                .set_property("Domain", &mut DispatchValue::string(domain))
                .windows_context("setting the RDP domain")?;
        }

        let width = config
            .document
            .get_integer("desktopwidth")
            .unwrap_or(1280)
            .clamp(200, 8192);
        let height = config
            .document
            .get_integer("desktopheight")
            .unwrap_or(800)
            .clamp(200, 8192);
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
            .set_property(
                "FullScreen",
                &mut DispatchValue::boolean(config.fullscreen && !config.span),
            )
            .windows_context("setting full-screen mode")?;
        if let Some(value) = config.document.get_integer("use multimon") {
            let multimon: IMsRdpClientNonScriptable5 = self
                .client
                .cast()
                .windows_context("opening the multiple-monitor RDP interface")?;
            unsafe { multimon.set_use_multimon(value != 0) }
                .windows_context("setting multiple-monitor mode")?;
        }

        if let Some(password) = config.password.as_ref() {
            let non_scriptable: IMsTscNonScriptable = self
                .client
                .cast()
                .windows_context("opening the secure RDP credential interface")?;
            let password = BSTR::from(password.expose());
            unsafe { non_scriptable.set_clear_text_password(&password) }
                .windows_context("passing the password to the system RDP control")?;
        }

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

        set_optional(
            &advanced,
            "ContainerHandledFullScreen",
            DispatchValue::boolean(config.span),
        );

        if let Some(port) = port {
            set_optional(
                &advanced,
                "RDPPort",
                DispatchValue::integer(i32::from(port)),
            );
        }
        if config.document.get_integer("redirectwebauthn") == Some(1) {
            if let Some(plugin) = system_webauthn_plugin_path() {
                set_optional(&advanced, "PluginDlls", DispatchValue::string(&plugin));
            } else {
                tracing::warn!(
                    "WebAuthn redirection was requested but the system WebAuthn RDP plug-in is unavailable"
                );
            }
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

        self.apply_extended_settings(config)?;
        self.apply_resource_redirection(false)?;
        self.apply_gateway_settings(config);
        self.apply_remote_app_settings(config);
        Ok(())
    }

    fn apply_extended_settings(&self, config: &SessionConfig) -> Result<()> {
        let extended: IMsRdpExtendedSettings = self
            .client
            .cast()
            .windows_context("opening the extended RDP settings")?;

        for (property, key) in [
            ("RestrictedLogon", "restricted admin mode"),
            ("RedirectedAuthentication", "remote credential guard"),
            ("EnableLocationRedirection", "redirectlocation"),
            ("EnableRdsAadAuth", "enablerdsaadauth"),
            ("RDGIsKDCProxy", "rdgiskdcproxy"),
        ] {
            if let Some(value) = config.document.get_integer(key) {
                set_extended_optional(&extended, property, DispatchValue::boolean(value != 0));
            }
        }
        for (property, key) in [
            ("DesktopScaleFactor", "desktopscalefactor"),
            ("DeviceScaleFactor", "devicescalefactor"),
        ] {
            if let Some(value) = config.document.get_integer(key).filter(|value| *value >= 0) {
                set_extended_optional(&extended, property, DispatchValue::unsigned(value as u32));
            }
        }
        for (property, key) in [
            ("SelectedMonitors", "selectedmonitors"),
            ("KDCProxyName", "kdcproxyname"),
        ] {
            if let Some(value) = config.document.get_string(key) {
                set_extended_optional(&extended, property, DispatchValue::string(value));
            }
        }
        Ok(())
    }

    fn apply_resource_redirection(&self, redirect_now: bool) -> Result<()> {
        if self.redirection.drives.is_some() || self.redirection.has_device_configuration() {
            let non_scriptable: IMsRdpClientNonScriptable3 = self
                .client
                .cast()
                .windows_context("opening the device-redirection RDP interface")?;

            if self.redirection.drives.is_some() {
                unsafe {
                    non_scriptable
                        .set_redirect_dynamic_drives(self.redirection.redirect_dynamic_drives())
                        .windows_context("setting dynamic drive redirection")?;
                }
                let drives = unsafe { non_scriptable.drive_collection() }
                    .windows_context("opening the redirected-drive collection")?;
                unsafe { drives.rescan(false) }
                    .windows_context("enumerating local drives for redirection")?;
                let count = unsafe { drives.count() }
                    .windows_context("counting local drives for redirection")?
                    .min(4_096);
                for index in 0..count {
                    let drive = unsafe { drives.by_index(index) }
                        .windows_context("opening a local drive redirection entry")?;
                    let name =
                        unsafe { drive.name() }.windows_context("reading a local drive name")?;
                    if let Some(selected) = self.redirection.drive_is_selected(&name) {
                        unsafe { drive.set_redirected(selected) }
                            .windows_context("selecting a local drive for redirection")?;
                    }
                }
            }

            if self.redirection.has_device_configuration() {
                unsafe {
                    non_scriptable
                        .set_redirect_dynamic_devices(self.redirection.redirect_dynamic_devices())
                        .windows_context("setting dynamic device redirection")?;
                }
                let devices = unsafe { non_scriptable.device_collection() }
                    .windows_context("opening the redirected-device collection")?;
                unsafe { devices.rescan(false) }
                    .windows_context("enumerating local devices for redirection")?;
                let count = unsafe { devices.count() }
                    .windows_context("counting local devices for redirection")?
                    .min(4_096);
                for index in 0..count {
                    let device = unsafe { devices.by_index(index) }
                        .windows_context("opening a local device redirection entry")?;
                    let instance_id = unsafe { device.instance_id() }
                        .windows_context("reading a local device instance identifier")?;
                    let is_usb = device
                        .cast::<IMsRdpDeviceV2>()
                        .ok()
                        .and_then(|device| unsafe { device.is_usb_device() }.ok())
                        .unwrap_or_else(|| {
                            instance_id
                                .get(..4)
                                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("USB\\"))
                                || instance_id.get(..8).is_some_and(|prefix| {
                                    prefix.eq_ignore_ascii_case("\\\\?\\usb#")
                                })
                        });
                    if let Some(selected) = self.redirection.device_is_selected(
                        &instance_id,
                        is_usb,
                        device_class_guid(&instance_id),
                    ) {
                        unsafe { device.set_redirected(selected) }
                            .windows_context("selecting a local device for redirection")?;
                    }
                }
                if redirect_now
                    && let Ok(devices_v2) = devices.cast::<IMsRdpDeviceCollection2>()
                    && let Err(error) = unsafe { devices_v2.redirect_now() }
                {
                    tracing::debug!(%error, "immediate device redirection is unavailable");
                }
            }
        }

        if self.redirection.has_camera_configuration() {
            let non_scriptable: IMsRdpClientNonScriptable7 = self
                .client
                .cast()
                .windows_context("opening the camera-redirection RDP interface")?;
            let cameras = unsafe { non_scriptable.camera_collection() }
                .windows_context("opening the camera-redirection collection")?;
            unsafe { cameras.rescan() }
                .windows_context("enumerating local cameras for redirection")?;

            if let Some(configured) = self.redirection.cameras.as_deref() {
                unsafe {
                    cameras
                        .set_redirect_by_default(
                            selectors(configured).any(|selector| selector == "*"),
                        )
                        .windows_context("setting the default camera-redirection state")?;
                }
                let count = unsafe { cameras.count() }
                    .windows_context("counting local cameras for redirection")?
                    .min(4_096);
                for index in 0..count {
                    let camera = unsafe { cameras.by_index(index) }
                        .windows_context("opening a local camera redirection entry")?;
                    let symbolic_link = unsafe { camera.symbolic_link() }
                        .windows_context("reading a local camera symbolic link")?;
                    if let Some(selected) = self.redirection.camera_is_selected(&symbolic_link) {
                        unsafe { camera.set_redirected(selected) }
                            .windows_context("selecting a local camera for redirection")?;
                    }
                }
            }
            if let Some(enabled) = self.redirection.encode_video {
                unsafe { cameras.set_encode_video(enabled) }
                    .windows_context("setting redirected-camera video encoding")?;
            }
            if let Some(quality) = self.redirection.encoding_quality {
                unsafe { cameras.set_encoding_quality(quality) }
                    .windows_context("setting redirected-camera encoding quality")?;
            }
        }
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
            let callback: IMsTscAxEvents = ComObject::new(EventCallback {
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

    pub fn update_display(
        &self,
        width: u32,
        height: u32,
        dpi: u32,
        document: &RdpDocument,
    ) -> bool {
        if width == 0 || height == 0 {
            return false;
        }
        let settings = SessionDisplaySettings::from_window(width, height, dpi, document);
        if let Err(error) = self
            .client
            .invoke_with_arguments("UpdateSessionDisplaySettings", settings.arguments())
        {
            // SmartSizing remains enabled, so older servers and controls still
            // resize cleanly without exposing scroll bars or clipped content.
            tracing::debug!(%error, "live RDP display update is unavailable; using SmartSizing");
            false
        } else {
            true
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

    pub fn notify_device_change(&self, wparam: WPARAM, lparam: LPARAM, refresh: bool) {
        match self.client.cast::<IMsRdpClientNonScriptable>() {
            Ok(non_scriptable) => {
                if let Err(error) =
                    unsafe { non_scriptable.notify_redirect_device_change(wparam, lparam) }
                {
                    tracing::debug!(%error, "forwarding the device-change notification failed");
                }
            }
            Err(error) => {
                tracing::debug!(%error, "device-change forwarding is unavailable");
            }
        }
        if refresh && let Err(error) = self.apply_resource_redirection(true) {
            tracing::warn!(%error, "refreshing redirected devices after a device change failed");
        }
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
    use std::time::{Duration, Instant};

    use windows::Win32::Foundation::{E_FAIL, HWND, S_FALSE, S_OK};
    use windows::Win32::System::Com::ITypeLib;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Ole::{LoadRegTypeLib, OleInitialize, OleUninitialize};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE, PeekMessageW,
        TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WS_OVERLAPPED, WS_VISIBLE,
    };
    use windows::core::{BSTR, GUID, Interface, w};

    use crate::config::{ConnectionOverrides, SecretString, SessionConfig};
    use crate::rdp::RdpDocument;

    use super::{
        ActiveXHost, DispatchExt, DispatchValue, IMsRdpClientNonScriptable5,
        IMsRdpExtendedSettings, RdpEventKind, SessionDisplaySettings, accelerator_was_handled,
    };

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
    fn system_rdp_event_dispids_match_dispatch_map() {
        const LIBID_MS_TSC_AX: GUID = GUID::from_u128(0x8c11efa1_92c3_11d1_bc1e_00c04fa31489);

        unsafe { OleInitialize(None) }.expect("OLE initialization must succeed");
        let _ole = OleGuard;
        let type_library = unsafe { LoadRegTypeLib(&LIBID_MS_TSC_AX, 1, 0, 0) }
            .expect("the system RDP type library must be registered");
        let event_type = unsafe { type_library.GetTypeInfoOfGuid(&super::DIID_MS_TSC_AX_EVENTS) }
            .expect("the RDP event dispinterface must have type information");

        for (dispid, expected_name, expected_kind) in [
            (1, "OnConnecting", RdpEventKind::Connecting),
            (2, "OnConnected", RdpEventKind::Connected),
            (3, "OnLoginComplete", RdpEventKind::LoginCompleted),
            (4, "OnDisconnected", RdpEventKind::Disconnected),
            (
                12,
                "OnRemoteDesktopSizeChange",
                RdpEventKind::RemoteDesktopSizeChanged,
            ),
            (17, "OnAutoReconnecting", RdpEventKind::AutoReconnecting),
            (
                18,
                "OnAuthenticationWarningDisplayed",
                RdpEventKind::DialogDisplaying,
            ),
            (
                19,
                "OnAuthenticationWarningDismissed",
                RdpEventKind::DialogDismissed,
            ),
            (22, "OnLogonError", RdpEventKind::LogonError),
            (
                29,
                "OnRemoteWindowDisplayed",
                RdpEventKind::RemoteProgramDisplayed,
            ),
            (
                32,
                "OnNetworkStatusChanged",
                RdpEventKind::NetworkStatusChanged,
            ),
            (33, "OnAutoReconnected", RdpEventKind::AutoReconnected),
            (34, "OnAutoReconnecting2", RdpEventKind::AutoReconnecting),
        ] {
            let mut name = BSTR::new();
            unsafe {
                event_type.GetDocumentation(
                    dispid,
                    Some(&mut name),
                    None,
                    std::ptr::null_mut(),
                    None,
                )
            }
            .unwrap_or_else(|error| panic!("DISPID {dispid} must be documented: {error}"));
            assert_eq!(name.to_string(), expected_name);
            assert_eq!(super::legacy_event_kind(dispid), Some(expected_kind));
        }
        assert_eq!(super::legacy_event_kind(31), None);
    }

    #[test]
    fn system_multimon_interface_layout_matches_binding() {
        const LIBID_MS_TSC_AX: GUID = GUID::from_u128(0x8c11efa1_92c3_11d1_bc1e_00c04fa31489);
        const IID_MS_RDP_CLIENT_NON_SCRIPTABLE_5: GUID =
            GUID::from_u128(0x4f6996d5_d7b1_412c_b0ff_063718566907);

        unsafe { OleInitialize(None) }.expect("OLE initialization must succeed");
        let _ole = OleGuard;
        let type_library = unsafe { LoadRegTypeLib(&LIBID_MS_TSC_AX, 1, 0, 0) }
            .expect("the system RDP type library must be registered");
        let type_info =
            unsafe { type_library.GetTypeInfoOfGuid(&IID_MS_RDP_CLIENT_NON_SCRIPTABLE_5) }
                .expect("the multiple-monitor interface must have type information");
        let type_attributes = unsafe { type_info.GetTypeAttr() }
            .expect("the multiple-monitor interface must expose type attributes");
        let function_count = unsafe { (*type_attributes).cFuncs };
        let mut use_multimon_offsets = Vec::new();
        for index in 0..u32::from(function_count) {
            let description = unsafe { type_info.GetFuncDesc(index) }
                .expect("the interface function must have a description");
            let mut name = BSTR::new();
            unsafe {
                type_info.GetDocumentation(
                    (*description).memid,
                    Some(&mut name),
                    None,
                    std::ptr::null_mut(),
                    None,
                )
            }
            .expect("the interface function must be documented");
            if name == "UseMultimon" {
                use_multimon_offsets.push(unsafe { (*description).oVft });
            }
            unsafe { type_info.ReleaseFuncDesc(description) };
        }
        unsafe { type_info.ReleaseTypeAttr(type_attributes) };
        assert_eq!(use_multimon_offsets, [424, 432]);
    }

    fn method_offsets(type_library: &ITypeLib, iid: &GUID, method: &str) -> Vec<i16> {
        let type_info = unsafe { type_library.GetTypeInfoOfGuid(iid) }
            .unwrap_or_else(|error| panic!("RDP interface {iid:?} must be registered: {error}"));
        let attributes =
            unsafe { type_info.GetTypeAttr() }.expect("RDP interface must expose type attributes");
        let mut offsets = Vec::new();
        for index in 0..u32::from(unsafe { (*attributes).cFuncs }) {
            let function = unsafe { type_info.GetFuncDesc(index) }
                .expect("RDP interface function must have a description");
            let mut name = BSTR::new();
            unsafe {
                type_info.GetDocumentation(
                    (*function).memid,
                    Some(&mut name),
                    None,
                    std::ptr::null_mut(),
                    None,
                )
            }
            .expect("RDP interface function must be documented");
            if name == method {
                offsets.push(unsafe { (*function).oVft });
            }
            unsafe { type_info.ReleaseFuncDesc(function) };
        }
        unsafe { type_info.ReleaseTypeAttr(attributes) };
        offsets
    }

    #[test]
    fn system_redirection_interface_layouts_match_bindings() {
        const LIBID_MS_TSC_AX: GUID = GUID::from_u128(0x8c11efa1_92c3_11d1_bc1e_00c04fa31489);

        unsafe { OleInitialize(None) }.expect("OLE initialization must succeed");
        let _ole = OleGuard;
        let type_library = unsafe { LoadRegTypeLib(&LIBID_MS_TSC_AX, 1, 0, 0) }
            .expect("the system RDP type library must be registered");

        for (iid, method, offsets) in [
            (
                super::IMsRdpClientNonScriptable::IID,
                "NotifyRedirectDeviceChange",
                vec![104],
            ),
            (
                super::IMsRdpClientNonScriptable3::IID,
                "RedirectDynamicDrives",
                vec![200, 208],
            ),
            (
                super::IMsRdpClientNonScriptable3::IID,
                "RedirectDynamicDevices",
                vec![216, 224],
            ),
            (
                super::IMsRdpClientNonScriptable3::IID,
                "DeviceCollection",
                vec![232],
            ),
            (
                super::IMsRdpClientNonScriptable3::IID,
                "DriveCollection",
                vec![240],
            ),
            (
                super::IMsRdpClientNonScriptable7::IID,
                "CameraRedirConfigCollection",
                vec![536],
            ),
            (
                super::IMsRdpDeviceCollection::IID,
                "RescanDevices",
                vec![24],
            ),
            (
                super::IMsRdpDeviceCollection::IID,
                "DeviceByIndex",
                vec![32],
            ),
            (super::IMsRdpDeviceCollection::IID, "DeviceCount", vec![48]),
            (super::IMsRdpDriveCollection::IID, "RescanDrives", vec![24]),
            (super::IMsRdpDriveCollection::IID, "DriveByIndex", vec![32]),
            (super::IMsRdpDriveCollection::IID, "DriveCount", vec![40]),
            (
                super::IMsRdpCameraRedirConfigCollection::IID,
                "Rescan",
                vec![24],
            ),
            (
                super::IMsRdpCameraRedirConfigCollection::IID,
                "Count",
                vec![32],
            ),
            (
                super::IMsRdpCameraRedirConfigCollection::IID,
                "ByIndex",
                vec![40],
            ),
            (
                super::IMsRdpCameraRedirConfigCollection::IID,
                "RedirectByDefault",
                vec![72, 80],
            ),
            (
                super::IMsRdpCameraRedirConfigCollection::IID,
                "EncodeVideo",
                vec![88, 96],
            ),
            (
                super::IMsRdpCameraRedirConfigCollection::IID,
                "EncodingQuality",
                vec![104, 112],
            ),
            (
                super::IMsRdpCameraRedirConfig::IID,
                "SymbolicLink",
                vec![32],
            ),
            (
                super::IMsRdpCameraRedirConfig::IID,
                "Redirected",
                vec![56, 64],
            ),
        ] {
            assert_eq!(
                method_offsets(&type_library, &iid, method),
                offsets,
                "{method}"
            );
        }
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
    fn resource_selectors_support_wildcards_dynamic_entries_classes_and_exclusions() {
        let class_guid = GUID::from_u128(0x12345678_1234_1234_1234_1234567890ab);
        assert!(super::selector_list_matches(
            "*;-USB\\VID_BLOCKED",
            "USB\\VID_ALLOWED&PID_0001",
            None,
            "dynamicdevices",
        ));
        assert!(!super::selector_list_matches(
            "*;-USB\\VID_BLOCKED",
            "USB\\VID_BLOCKED&PID_0002",
            None,
            "dynamicdevices",
        ));
        assert!(super::selector_list_matches(
            "{12345678-1234-1234-1234-1234567890ab}",
            "ROOT\\SAMPLE",
            Some(class_guid),
            "dynamicdevices",
        ));
        assert!(!super::selector_list_matches(
            "DynamicDevices",
            "ROOT\\SAMPLE",
            None,
            "dynamicdevices",
        ));
        assert!(!super::selector_list_matches(
            "*;-*",
            "ROOT\\SAMPLE",
            None,
            "dynamicdevices",
        ));

        let settings = super::RedirectionSettings {
            drives: Some("C:\\;DynamicDrives".to_owned()),
            ..Default::default()
        };
        assert_eq!(settings.drive_is_selected("C:\\"), Some(true));
        assert_eq!(settings.drive_is_selected("D:\\"), Some(false));
        assert!(settings.redirect_dynamic_drives());
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

            let mut host = ActiveXHost::activate(window.0)
                .expect("the system Remote Desktop ActiveX control must activate");
            host.attach_events(window.0);
            assert!(
                host.event_connection.is_some(),
                "the RDP control must accept its outgoing event dispinterface"
            );
            host.client
                .dispatch_id("UpdateSessionDisplaySettings")
                .expect("the version 9/10 RDP control must expose live display updates");
            let extended: IMsRdpExtendedSettings = host
                .client
                .cast()
                .expect("the version 9/10 RDP control must expose extended settings");
            let advanced = host
                .client
                .get_dispatch(&["AdvancedSettings9", "AdvancedSettings8"])
                .expect("advanced settings must be exposed");
            let plugin = super::system_webauthn_plugin_path()
                .expect("the serviced Windows WebAuthn RDP plug-in must exist");
            advanced
                .set_property("PluginDlls", &mut DispatchValue::string(&plugin))
                .expect("the RDP control must accept the system WebAuthn plug-in");
            let name = BSTR::from("SelectedMonitors");
            let mut selected_monitors = DispatchValue::string("0");
            unsafe { extended.set_named_property(&name, &mut selected_monitors.0) }
                .expect("the RDP control must accept selected monitor configuration");
            let multimon: IMsRdpClientNonScriptable5 = host
                .client
                .cast()
                .expect("the RDP control must expose multiple-monitor configuration");
            unsafe { multimon.set_use_multimon(true) }
                .expect("the RDP control must accept multiple-monitor mode");
            let prompted_config = SessionConfig::resolve(
                None,
                ConnectionOverrides {
                    server: Some("localhost:3389".to_owned()),
                    ..Default::default()
                },
            )
            .expect("prompted RDP settings must resolve");
            host.apply_settings(&prompted_config)
                .expect("the RDP control must allow the system credential prompt");
            let config = SessionConfig::resolve(
                None,
                ConnectionOverrides {
                    server: Some("localhost:3389".to_owned()),
                    username: Some("mstsc-rs-ci".to_owned()),
                    password: Some(SecretString::new("not-a-real-password")),
                    multimon: Some(true),
                    restricted_admin: Some(true),
                    redirect_location: Some(true),
                    custom_properties: vec![
                        "selectedmonitors:s:0".to_owned(),
                        "drivestoredirect:s:".to_owned(),
                        "devicestoredirect:s:".to_owned(),
                        "usbdevicestoredirect:s:".to_owned(),
                        "camerastoredirect:s:".to_owned(),
                        "encode redirected video capture:i:1".to_owned(),
                        "redirected video capture encoding quality:i:1".to_owned(),
                    ],
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

    #[test]
    #[ignore = "requires an explicitly configured live RDP test host"]
    fn live_rdp_session_reaches_login_complete() {
        std::thread::spawn(|| {
            unsafe { OleInitialize(None) }.expect("OLE initialization must succeed");
            let _ole = OleGuard;

            let server = std::env::var("MSTSC_RS_TEST_SERVER")
                .expect("MSTSC_RS_TEST_SERVER must name the live test host");
            let username = std::env::var("MSTSC_RS_TEST_USERNAME")
                .expect("MSTSC_RS_TEST_USERNAME must name the live test account");
            let domain = std::env::var("MSTSC_RS_TEST_DOMAIN").ok();
            let password = std::env::var("MSTSC_RS_TEST_PASSWORD")
                .expect("MSTSC_RS_TEST_PASSWORD must contain the live test password");
            let timeout = std::env::var("MSTSC_RS_TEST_TIMEOUT_SECONDS")
                .ok()
                .and_then(|value| value.parse().ok())
                .map(Duration::from_secs)
                .unwrap_or(Duration::from_secs(45));
            let fullscreen = std::env::var("MSTSC_RS_TEST_FULLSCREEN")
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
            let redirect_webauthn = std::env::var("MSTSC_RS_TEST_WEBAUTHN").map_or(true, |value| {
                value != "0" && !value.eq_ignore_ascii_case("false")
            });
            let redirect_test_devices = std::env::var("MSTSC_RS_TEST_REDIRECTION")
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
            let mut custom_properties = vec!["authentication level:i:0".to_owned()];
            if redirect_test_devices {
                custom_properties.extend([
                    "drivestoredirect:s:*".to_owned(),
                    "devicestoredirect:s:*".to_owned(),
                    "usbdevicestoredirect:s:*".to_owned(),
                    "camerastoredirect:s:*".to_owned(),
                    "encode redirected video capture:i:1".to_owned(),
                    "redirected video capture encoding quality:i:1".to_owned(),
                ]);
            }

            let hwnd = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("STATIC"),
                    w!("mstsc-rs live connection test"),
                    WINDOW_STYLE(WS_OVERLAPPED.0 | WS_VISIBLE.0),
                    0,
                    0,
                    1024,
                    768,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .expect("live test host window must be created");
            let window = WindowGuard(hwnd);
            let config = SessionConfig::resolve(
                None,
                ConnectionOverrides {
                    server: Some(server),
                    username: Some(username),
                    domain,
                    password: Some(SecretString::new(password)),
                    fullscreen: fullscreen.then_some(true),
                    redirect_clipboard: Some(false),
                    redirect_printers: Some(false),
                    redirect_smartcards: Some(false),
                    redirect_microphone: Some(false),
                    redirect_webauthn: Some(redirect_webauthn),
                    custom_properties,
                    ..Default::default()
                },
            )
            .expect("live RDP settings must resolve");
            let host = ActiveXHost::create(window.0, &config)
                .expect("the system RDP control must start the live connection");

            let started = Instant::now();
            let mut login_complete = false;
            while started.elapsed() < timeout && !login_complete {
                let mut message = MSG::default();
                while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                    unsafe {
                        let _ = TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                for event in host.take_events() {
                    eprintln!("live RDP event: {:?} {:?}", event.kind, event.arguments);
                    match event.kind {
                        RdpEventKind::LoginCompleted => login_complete = true,
                        RdpEventKind::Disconnected => {
                            panic!("live RDP session disconnected: {:?}", event.arguments)
                        }
                        _ => {}
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }

            assert!(
                login_complete,
                "live RDP session did not reach OnLoginComplete within {timeout:?}"
            );
            assert_eq!(
                unsafe { GetModuleHandleW(w!("webauthn.dll")) }.is_ok(),
                redirect_webauthn,
                "the system WebAuthn DVC plug-in load state must follow redirectwebauthn"
            );
            if !fullscreen {
                host.update_display(1000, 700, 96, &config.document);
                let resize_started = Instant::now();
                let mut next_resize_attempt = resize_started + Duration::from_secs(1);
                let mut resized = false;
                while resize_started.elapsed() < Duration::from_secs(10) && !resized {
                    if Instant::now() >= next_resize_attempt {
                        host.update_display(1000, 700, 96, &config.document);
                        next_resize_attempt += Duration::from_secs(1);
                    }
                    let mut message = MSG::default();
                    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                        unsafe {
                            let _ = TranslateMessage(&message);
                            DispatchMessageW(&message);
                        }
                    }
                    for event in host.take_events() {
                        eprintln!(
                            "live RDP resize event: {:?} {:?}",
                            event.kind, event.arguments
                        );
                        if event.kind == RdpEventKind::RemoteDesktopSizeChanged
                            && event.arguments.first().is_some_and(|value| value == "1000")
                            && event.arguments.get(1).is_some_and(|value| value == "700")
                        {
                            resized = true;
                        } else if event.kind == RdpEventKind::Disconnected {
                            panic!("live RDP session disconnected: {:?}", event.arguments);
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                assert!(
                    resized,
                    "live RDP session did not resize to 1000x700 within 10 seconds"
                );
            }
            host.disconnect();
            drop(host);
            drop(window);
        })
        .join()
        .expect("live RDP test thread must not panic");
    }
}
