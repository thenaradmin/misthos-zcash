//! On-chain lookup for `verify-proof --lwd`: fetch the raw tx and its mined height
//! from a lightwalletd server instead of trusting operator-supplied bytes and height.

use zcash_client_backend::proto::service::{compact_tx_streamer_client::CompactTxStreamerClient, TxFilter};

pub struct OnChain {
    pub raw: Vec<u8>,
    /// Mined height, or None if lightwalletd reports it as mempool / not in the best chain.
    pub height: Option<u32>,
}

pub fn fetch_tx(lwd_url: &str, txid_hex: &str) -> Result<OnChain, String> {
    let mut hash = hex::decode(txid_hex.trim()).map_err(|e| format!("txid not hex: {e}"))?;
    if hash.len() != 32 {
        return Err("txid must be 32 bytes".into());
    }
    hash.reverse(); // display order -> internal byte order
    let _ = rustls::crypto::ring::default_provider().install_default(); // Err = already installed
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|e| e.to_string())?;
    rt.block_on(async {
        let ep = tonic::transport::Endpoint::from_shared(lwd_url.to_string())
            .map_err(|e| e.to_string())?
            .tls_config(tonic::transport::ClientTlsConfig::new().with_webpki_roots())
            .map_err(|e| e.to_string())?
            .timeout(std::time::Duration::from_secs(30));
        let ch = ep.connect().await.map_err(|e| format!("lightwalletd {lwd_url}: {e}"))?;
        let mut c = CompactTxStreamerClient::new(ch);
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
