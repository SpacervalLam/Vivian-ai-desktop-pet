//! 网络连接状态感知 —— 通过 COM INetworkListManager 获取当前网络连接类型与名称。

use serde::{Deserialize, Serialize};

/// 网络连接状态快照
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NetworkStatusSnapshot {
    /// 是否已连接到互联网
    pub connected: bool,
    /// 连接名称（如 Wi-Fi SSID 或以太网名称）
    pub name: Option<String>,
    /// 接口类型（Wi-Fi / Ethernet / 未知）
    pub interface_type: Option<String>,
}

/// 获取当前网络连接状态（Windows，通过 COM INetworkListManager）。
///
/// 非 Windows 平台返回默认值（connected=false）。
pub fn get_network_status() -> NetworkStatusSnapshot {
    #[cfg(target_os = "windows")]
    {
        try_get_network_windows().unwrap_or_default()
    }
    #[cfg(not(target_os = "windows"))]
    {
        NetworkStatusSnapshot::default()
    }
}

#[cfg(target_os = "windows")]
fn try_get_network_windows() -> Option<NetworkStatusSnapshot> {
    use windows::Win32::Networking::NetworkListManager::{
        INetwork, INetworkListManager, NetworkListManager,
        NLM_ENUM_NETWORK_CONNECTED,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let manager: INetworkListManager =
            CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok()?;

        let connected = manager.IsConnectedToInternet().unwrap_or_default().as_bool();
        if !connected {
            return Some(NetworkStatusSnapshot {
                connected: false,
                name: None,
                interface_type: None,
            });
        }

        // 获取已连接网络的名称
        let mut name = None;
        if let Ok(networks) = manager.GetNetworks(NLM_ENUM_NETWORK_CONNECTED) {
            let mut items: [Option<INetwork>; 1] = [None];
            if networks.Next(&mut items, None).is_ok() {
                if let Some(net) = items[0].take() {
                    if let Ok(n) = net.GetName() {
                        name = Some(n.to_string());
                    }
                }
            }
        }

        Some(NetworkStatusSnapshot {
            connected: true,
            name,
            interface_type: query_interface_type(&manager),
        })
    }
}

/// 通过 INetworkConnection::GetAdapterId 拿到适配器 GUID，
/// 再经 IP Helper API（ConvertInterfaceGuidToLuid → GetIfEntry2）查询接口类型，
/// 映射为 Wi-Fi / Ethernet / Unknown。
#[cfg(target_os = "windows")]
fn query_interface_type(
    manager: &windows::Win32::Networking::NetworkListManager::INetworkListManager,
) -> Option<String> {
    use windows::Win32::NetworkManagement::IpHelper::{
        ConvertInterfaceGuidToLuid, GetIfEntry2, MIB_IF_ROW2,
    };
    use windows::Win32::NetworkManagement::Ndis::NET_LUID_LH;
    use windows::Win32::Networking::NetworkListManager::INetworkConnection;

    const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
    const IF_TYPE_IEEE80211: u32 = 71;
    const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

    unsafe {
        let connections = manager.GetNetworkConnections().ok()?;
        let mut items: [Option<INetworkConnection>; 1] = [None];
        connections.Next(&mut items, None).ok()?;
        let conn = items[0].take()?;
        let adapter_guid = conn.GetAdapterId().ok()?;

        let mut luid: NET_LUID_LH = std::mem::zeroed();
        if ConvertInterfaceGuidToLuid(&adapter_guid, &mut luid).0 != 0 {
            return None;
        }

        let mut row: MIB_IF_ROW2 = std::mem::zeroed();
        row.InterfaceLuid = luid;
        if GetIfEntry2(&mut row).0 != 0 {
            return None;
        }

        Some(
            match row.Type {
                IF_TYPE_ETHERNET_CSMACD => "Ethernet",
                IF_TYPE_IEEE80211 => "Wi-Fi",
                IF_TYPE_SOFTWARE_LOOPBACK => "Loopback",
                _ => "Unknown",
            }
            .to_string(),
        )
    }
}
