//! Direct lightwalletd reads: the raw tx + mined height for `verify-proof --lwd` and
//! for `pay --two-step` (funding confirmation), and the chain tip for the payout delays.

use zcash_client_backend::proto::service::{compact_tx_streamer_client::CompactTxStreamerClient, ChainSpec, TxFilter};

type Client = CompactTxStreamerClient<tonic::transport::Channel>;

pub struct OnChain {
    pub raw: Vec<u8>,
    /// Mined height, or None if lightwalletd reports it as mempool / not in the best chain.
    pub height: Option<u32>,
}

/// One-shot gRPC call on a fresh current-thread runtime.
fn with_client<T>(lwd_url: &str, f: impl AsyncFnOnce(&mut Client) -> Result<T, String>) -> Result<T, String> {
    let _ = rustls::crypto::ring::default_provider().install_default(); // Err = already installed
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let ep = tonic::transport::Endpoint::from_shared(lwd_url.to_string())
            .map_err(|e| e.to_string())?
            .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| e.to_string())?
            .timeout(std::time::Duration::from_secs(30));
        let ch = ep.connect().await.map_err(|e| format!("lightwalletd {lwd_url}: {e}"))?;
        f(&mut CompactTxStreamerClient::new(ch)).await
    })
}

pub fn fetch_tx(lwd_url: &str, txid_hex: &str) -> Result<OnChain, String> {
    let mut hash = hex::decode(txid_hex.trim()).map_err(|e| format!("txid not hex: {e}"))?;
    if hash.len() != 32 {
        return Err("txid must be 32 bytes".into());
    }
    hash.reverse(); // display order -> internal byte order
    with_client(lwd_url, async |c| {
        let r = c
            .get_transaction(TxFilter { block: None, index: 0, hash })
            .await
            .map_err(|e| format!("lightwalletd GetTransaction: {}", e.message()))?
            .into_inner();
        // 0 = mempool, u64::MAX (or u32::MAX) = not in the main chain.
        let height = match r.height {
            0 => None,
            h if h >= u64::from(u32::MAX) => None,
            h => Some(h as u32),
        };
        Ok(OnChain { raw: r.data, height })
    })
}

/// Current best-chain tip height (`GetLatestBlock`). Reveals nothing about the wallet.
pub fn tip_height(lwd_url: &str) -> Result<u32, String> {
    with_client(lwd_url, async |c| {
        let b = c
            .get_latest_block(ChainSpec {})
            .await
            .map_err(|e| format!("lightwalletd GetLatestBlock: {}", e.message()))?
            .into_inner();
        u32::try_from(b.height).map_err(|_| format!("lightwalletd reported impossible tip height {}", b.height))
    })
}
