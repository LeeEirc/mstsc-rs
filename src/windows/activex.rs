use std::mem::{ManuallyDrop, size_of};
use std::sync::{Arc, Mutex};

use windows::Security::Cryptography::DataProtection::DataProtectionProvider;
use windows::Security::Cryptography::{BinaryStringEncoding, CryptographicBuffer};
use windows::Win32::Foundation::{
    DISP_E_UNKNOWNNAME, E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, RECT, SIZE, WPARAM,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch,
    IDispatch_Impl, IPersistStreamInit, ITypeInfo,
};
use windows::Win32::System::Ole::{
    IOleClientSite, IOleClientSite_Impl, IOleContainer, IOleInPlaceActiveObject, IOleInPlaceFrame,
    IOleInPlaceFrame_Impl, IOleInPlaceObject, IOleInPlaceSite, IOleInPlaceSite_Impl,
    IOleInPlaceUIWindow, IOleInPlaceUIWindow_Impl, IOleObject, IOleWindow_Impl, OLECLOSE_NOSAVE,
    OLEGETMONIKER, OLEINPLACEFRAMEINFO, OLEIVERB_INPLACEACTIVATE, OLEMENUGROUPWIDTHS, OLEWHICHMK,
    OleSetContainedObject,
};
use windows::Win32::System::RemoteDesktop::IRemoteDesktopClient;
use windows::Win32::System::Variant::{
    VARIANT, VT_BSTR, VT_I2, VT_I4, VT_INT, VT_UI2, VT_UI4, VT_UINT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, HACCEL, HMENU, MSG, PostMessageW};
use windows::core::{
    BOOL, BSTR, ComObject, Error as WinError, GUID, HRESULT, HSTRING, IUnknown, IUnknownImpl,
    Interface, OutRef, PCWSTR, Ref,
};

use crate::config::SessionConfig;
use crate::error::{Result, WindowsContext};

use super::ui::WM_RDP_EVENT;

// CLSID_RemoteDesktopClient, built into mstscax.dll on Windows 10/11.
const CLSID_REMOTE_DESKTOP_CLIENT: GUID = GUID::from_u128(0xeab16c5d_eed1_4e95_868b_0fba1b42c092);

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
    StatusChanged,
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
    kind: RdpEventKind,
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
        let arguments = unsafe { dispatch_arguments(pdispparams) };
        if let Ok(mut events) = self.events.lock() {
            events.push(RdpEvent {
                kind: self.kind,
                arguments,
            });
            let _ = unsafe { PostMessageW(Some(self.hwnd), WM_RDP_EVENT, WPARAM(0), LPARAM(0)) };
        }
        Ok(())
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

pub(super) struct ActiveXHost {
    site: ComObject<ActiveXSite>,
    ole_object: IOleObject,
    inplace_object: Option<IOleInPlaceObject>,
    active_object: Option<IOleInPlaceActiveObject>,
    client: IRemoteDesktopClient,
    callbacks: Vec<(BSTR, IDispatch)>,
    events: Arc<Mutex<Vec<RdpEvent>>>,
}

impl ActiveXHost {
    pub fn create(hwnd: HWND, config: &SessionConfig) -> Result<Self> {
        let site = ComObject::new(ActiveXSite { hwnd });
        let unknown: IUnknown =
            unsafe { CoCreateInstance(&CLSID_REMOTE_DESKTOP_CLIENT, None, CLSCTX_INPROC_SERVER) }
                .windows_context(
                "creating the system RemoteDesktopClient control (mstscax.dll may be unavailable)",
            )?;
        let ole_object: IOleObject = unknown
            .cast()
            .windows_context("querying the RDP control's IOleObject interface")?;
        let client: IRemoteDesktopClient = unknown
            .cast()
            .windows_context("querying the RDP client interface")?;

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
            ole_object
                .DoVerb(
                    OLEIVERB_INPLACEACTIVATE.0,
                    std::ptr::null(),
                    &client_site,
                    0,
                    hwnd,
                    &rect,
                )
                .windows_context("activating the embedded RDP control")?;
        }

        let mut host = Self {
            site,
            inplace_object: unknown.cast().ok(),
            active_object: unknown.cast().ok(),
            ole_object,
            client,
            callbacks: Vec::new(),
            events: Arc::new(Mutex::new(Vec::new())),
        };

        let settings = unsafe { host.client.Settings() }
            .windows_context("opening the RDP settings interface")?;
        let mut document = config.document.clone();
        if let Some(password) = config.password.as_ref() {
            let encrypted = encrypt_password(password.expose())?;
            document.remove_all("password");
            document.remove_all("password 51");
            document.remove_all("WinRTPasswordEncoding");
            document.remove_all("WinRTEncryptedPassword");
            document.set_integer("WinRTPasswordEncoding", 1);
            document.set_string("WinRTEncryptedPassword", encrypted);
        }
        let settings_text = BSTR::from(document.render());
        unsafe { settings.ApplySettings(&settings_text) }
            .windows_context("applying the merged RDP settings")?;

        host.attach_events(hwnd);
        host.resize(rect);
        unsafe { host.client.Connect() }.windows_context("starting the RDP connection")?;
        Ok(host)
    }

    fn attach_events(&mut self, hwnd: HWND) {
        const EVENTS: &[(&str, RdpEventKind)] = &[
            ("OnConnecting", RdpEventKind::Connecting),
            ("OnConnected", RdpEventKind::Connected),
            ("OnLoginCompleted", RdpEventKind::LoginCompleted),
            ("OnDisconnected", RdpEventKind::Disconnected),
            ("OnDialogDisplaying", RdpEventKind::DialogDisplaying),
            ("OnDialogDismissed", RdpEventKind::DialogDismissed),
            ("OnNetworkStatusChanged", RdpEventKind::NetworkStatusChanged),
            (
                "OnRemoteDesktopSizeChanged",
                RdpEventKind::RemoteDesktopSizeChanged,
            ),
            ("OnStatusChanged", RdpEventKind::StatusChanged),
        ];

        for (name, kind) in EVENTS {
            let event_name = BSTR::from(*name);
            let callback: IDispatch = ComObject::new(EventCallback {
                hwnd,
                kind: *kind,
                events: self.events.clone(),
            })
            .into_interface();
            match unsafe { self.client.attachEvent(&event_name, &callback) } {
                Ok(()) => self.callbacks.push((event_name, callback)),
                Err(error) => tracing::debug!(event = *name, %error, "RDP event is unavailable"),
            }
        }
    }

    pub fn resize(&self, rect: RECT) {
        if let Some(inplace) = &self.inplace_object {
            let _ = unsafe { inplace.SetObjectRects(&rect, &rect) };
        }
    }

    pub fn update_display(&self, width: u32, height: u32) {
        if width > 0 && height > 0 {
            let _ = unsafe { self.client.UpdateSessionDisplaySettings(width, height) };
        }
    }

    pub fn translate_accelerator(&self, message: &MSG) -> bool {
        self.active_object
            .as_ref()
            .is_some_and(|active| unsafe { active.TranslateAccelerator(Some(message)) }.is_ok())
    }

    pub fn disconnect(&self) {
        let _ = unsafe { self.client.Disconnect() };
    }

    pub fn take_events(&self) -> Vec<RdpEvent> {
        self.events
            .lock()
            .map(|mut events| std::mem::take(&mut *events))
            .unwrap_or_default()
    }
}

impl Drop for ActiveXHost {
    fn drop(&mut self) {
        for (name, callback) in self.callbacks.drain(..) {
            let _ = unsafe { self.client.detachEvent(&name, &callback) };
        }
        let _ = unsafe { self.client.Disconnect() };
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

fn encrypt_password(password: &str) -> Result<String> {
    let value = HSTRING::from(password);
    let buffer = CryptographicBuffer::ConvertStringToBinary(&value, BinaryStringEncoding::Utf16LE)
        .windows_context("encoding the password")?;
    let provider = DataProtectionProvider::CreateOverloadExplicit(&HSTRING::from("LOCAL=user"))
        .windows_context("opening Windows data protection")?;
    let protected = provider
        .ProtectAsync(&buffer)
        .windows_context("starting password protection")?
        .join()
        .windows_context("protecting the password for the current Windows user")?;
    CryptographicBuffer::EncodeToBase64String(&protected)
        .map(|value| value.to_string())
        .windows_context("encoding the protected password")
}
