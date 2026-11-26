//! Touch ID (or the account password) before writes to production.

/// Ask the device owner to confirm. Blocks until they answer, so call it off
/// the main thread. `Err` holds the reason it didn't go through.
#[cfg(target_os = "macos")]
pub fn confirm(reason: &str) -> Result<(), String> {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};

    let (tx, rx) = std::sync::mpsc::channel();
    let context = unsafe { LAContext::new() };
    let policy = LAPolicy::DeviceOwnerAuthentication;
    if let Err(e) = unsafe { context.canEvaluatePolicy_error(policy) } {
        return Err(e.localizedDescription().to_string());
    }
    let reply = RcBlock::new(move |ok: Bool, err: *mut NSError| {
        let res = if ok.as_bool() {
            Ok(())
        } else {
            Err(unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string()).unwrap_or_else(|| "Authentication failed".into()))
        };
        let _ = tx.send(res);
    });
    unsafe { context.evaluatePolicy_localizedReason_reply(policy, &NSString::from_str(reason), &reply) };
    rx.recv().unwrap_or_else(|_| Err("Authentication was interrupted".into()))
}

#[cfg(not(target_os = "macos"))]
pub fn confirm(_reason: &str) -> Result<(), String> {
    Ok(())
}
