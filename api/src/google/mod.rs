pub mod id_token;
pub mod service_account;

#[cfg(test)]
pub(crate) mod test_keys {
    /// Throwaway RSA-2048 keypair, generated for these tests only and used nowhere else.
    pub const PRIVATE_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQDIZhPZnmmGG1cX\n\
xciHHVdn/wU/Iv1UcXWS+LazKXL+DAfrgQJecD0Le0zHUtBz7vvY/u9awzTxVWuH\n\
Rzr3S4SeLgyM7q2+uS/HawniMqTcNLpbfbp+TteKp0kbQc94rJ6ibGAngW6+94Rq\n\
NBtchKevTpxpGGELYeWG1nkRNvlpriMdNAYTpHu/CgoLt1OFXKUvBHgXiAkz0XM4\n\
95UFb/k801lsiH5+O+j+GxvE9NuTuQASNgqH7HU9qB5PslEhPF4cAyAOm0n3/xbK\n\
G3DfyEVjztIU8nbiKNV2YeId17GHV/XFDoJHfXahlGWCSZPl2U7Owa+q2vhKpTTF\n\
0h7E3KE9AgMBAAECggEADhl4fhAyNnhH+Wk2AaOj2bJMqTOKBV3yOsP6/sIE0DEC\n\
gaRTCVf2Q4+KSF/gQLfoykythIJFgFFVPnBZirqqewKcaXIFkWywGuTCDkMn7EJd\n\
dkBzVZ2wdpXsg9lugnwKsTzUyxLFOvRpQZum7Sx8K85pHM/VTBq4LnN6kyZ+B9xT\n\
xP8GWHeLGGLKKlR8mFfPnzL6Rwez2hRS/+N4ElVIx3NqmVIfajfbx41JBKEZEzxH\n\
dOU0kVtUd7oYMjBGBK2qVHrqfBTATbSL/iVbQr/qImzbH1kQq+SyeX3PLGOZCczW\n\
YXePIDr8+581x/MfcqCVDW4QAaM0chkrX39iv9TjAQKBgQDnXrOFS++zAAQ+AdRG\n\
fpYl4DURpqTOPNzunSPrKCpHlVQ4wZGyazRR/Nth/oXcHXc2UQkuN9iBRvAy4rSq\n\
k26t+Ma43met105ogdwPCl0n39d3c3Ub2/oLcqhC8G38p2JRM2RKMd/GtPq+eoE9\n\
6mg9Ir4VO/IWdHa8Kto84NywfQKBgQDdu1lJkQaWq+4rsSF1d8pUgHd8iYoSnTnb\n\
fBll62jxS1LSAuwYukthEp93C6jryiCMkAD5fBxKZ4CuncQtMO/jNRl7uwrkjiCR\n\
tO9pYN+Z+JzwIh4jgc3U8P/RIkQm1YdfCrNYzAvhor+pL9FAnwAMnnYBKSpj9RJa\n\
JMfmYClPwQKBgFDK4/tHAgPKnLCeaObY60t8mGt6epA99pQoMVO7adAnWAqdpWfd\n\
QZ7joiP9AfdUHr3L3F3USHkUMWpDef0akIqVlacWmrO8vHeH6TUjVFZJNu9SVPGO\n\
hikbN6BwXqfcN1vNi1bRRagYqHp9QsZBJXc7u46kqSuoLZIVkLGzf7ztAoGAG+6+\n\
EMZ0TgbauiZWULYGhx6n7shk3HNyFnRLvsspLTyP8mTr2TGezVvLOvP9E7ci4pJB\n\
jSxVZAol69l0HT1M+xJ1Y2Dsvl4pPDHO9zd886ieHKIvqclwZKL3lyD59OpxY0e1\n\
0DnkRUh7XJTph3qARxavRvYooO7+oNyBUkRK8AECgYEApj6vwZ6Cco+7ucDgJLnj\n\
bU361siUSPOtPZ5eyyQ1p7K4KwqYH2QE62zg2VTCYVFWUJWMMcgguVDl7cnILlsc\n\
srXx7tCHg2dnnfjPtNF7jCCMIhyKv/QLL6ECyK6TUWr9JY2YSV/sdjInNiNRrgBE\n\
dzr05M1QTjAJFUBIco44ALk=\n\
-----END PRIVATE KEY-----\n";

    pub const MODULUS_B64: &str = "yGYT2Z5phhtXF8XIhx1XZ_8FPyL9VHF1kvi2syly_gwH64ECXnA9C3tMx1LQc-772P7vWsM08VVrh0c690uEni4MjO6tvrkvx2sJ4jKk3DS6W326fk7XiqdJG0HPeKyeomxgJ4FuvveEajQbXISnr06caRhhC2HlhtZ5ETb5aa4jHTQGE6R7vwoKC7dThVylLwR4F4gJM9FzOPeVBW_5PNNZbIh-fjvo_hsbxPTbk7kAEjYKh-x1PageT7JRITxeHAMgDptJ9_8Wyhtw38hFY87SFPJ24ijVdmHiHdexh1f1xQ6CR312oZRlgkmT5dlOzsGvqtr4SqU0xdIexNyhPQ";

    pub const EXPONENT_B64: &str = "AQAB";

    pub fn service_account_json(client_email: &str, token_uri: &str) -> String {
        serde_json::json!({
            "type": "service_account",
            "project_id": "kobbler-test",
            "private_key_id": "test-key-id",
            "private_key": PRIVATE_KEY_PEM,
            "client_email": client_email,
            "token_uri": token_uri,
        })
        .to_string()
    }
}
