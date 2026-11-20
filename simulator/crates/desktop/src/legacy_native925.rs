use cs2_runtime::ResourceIndex;
use cs2_script::NativeReturn;

use super::particle_plugin_digest::sha256;

const HAPPINESS2_COMPANION_PE_SHA256: &str =
    "5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c";
const REBELLIONS_PE_SIZE: usize = 4_350_976;
const REBELLIONS_PE_SHA256: &str =
    "a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3";
const REBELLIONS_SSCRIPT_SHA256: &str =
    "533ecc0aec6ae5c0b6b2e21f56ee1d3e4637f5eb655ac0b746a133c06f323407";

/// Resolves only the zero-argument call after the host's companion verification.
pub(super) fn happiness2_companion_native(
    index: u32,
    arguments: &[u8],
    verified_companion: Option<&str>,
) -> Option<NativeReturn> {
    verified_zero_argument_native925(index, arguments, || {
        verified_companion == Some(HAPPINESS2_COMPANION_PE_SHA256)
    })
}

pub(super) fn rebellions_native(
    index: u32,
    arguments: &[u8],
    resources: &ResourceIndex,
) -> Option<NativeReturn> {
    rebellions_native_with_fingerprints(
        index,
        arguments,
        || {
            let runtime = resources.resolve_plain_file("REBELLIONS.exe").ok()??;
            if runtime.size != REBELLIONS_PE_SIZE as u64 {
                return None;
            }
            let bytes = resources
                .read_candidate_bounded(runtime, REBELLIONS_PE_SIZE)
                .ok()??;
            Some(sha256(&bytes))
        },
        || {
            let names = resources.category_names(b"kcs", "sscript").ok()?;
            let script = names.iter().find_map(|name| resources.find(name))?;
            Some(sha256(&resources.read_candidate(script).ok()?))
        },
    )
}

fn rebellions_native_with_fingerprints(
    index: u32,
    arguments: &[u8],
    runtime_sha256: impl FnOnce() -> Option<String>,
    script_sha256: impl FnOnce() -> Option<String>,
) -> Option<NativeReturn> {
    verified_zero_argument_native925(index, arguments, || {
        runtime_sha256().as_deref() == Some(REBELLIONS_PE_SHA256)
            && script_sha256().as_deref() == Some(REBELLIONS_SSCRIPT_SHA256)
    })
}

fn verified_zero_argument_native925(
    index: u32,
    arguments: &[u8],
    verified: impl FnOnce() -> bool,
) -> Option<NativeReturn> {
    (index == 925 && arguments.is_empty() && verified()).then_some(NativeReturn::Value(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn happiness2_native925_requires_verified_companion_and_zero_arguments() {
        let companion = Some(HAPPINESS2_COMPANION_PE_SHA256);
        let packed = Some("21dfeb88edafc4bf19226d158e040687738f592282c578740e9d972127938df7");
        assert!(matches!(
            happiness2_companion_native(925, &[], companion),
            Some(NativeReturn::Value(0))
        ));
        assert!(happiness2_companion_native(925, &[], None).is_none());
        assert!(happiness2_companion_native(925, &[], packed).is_none());
        assert!(happiness2_companion_native(925, &[0; 4], companion).is_none());
        assert!(happiness2_companion_native(924, &[], companion).is_none());
    }

    #[test]
    fn rebellions_native925_requires_verified_runtime_and_zero_arguments() {
        let runtime = || Some(REBELLIONS_PE_SHA256.to_owned());
        let script = || Some(REBELLIONS_SSCRIPT_SHA256.to_owned());
        assert_eq!(
            rebellions_native_with_fingerprints(925, &[], runtime, script),
            Some(NativeReturn::Value(0))
        );
        let companion = || Some(HAPPINESS2_COMPANION_PE_SHA256.to_owned());
        let unexpected_read = || -> Option<String> { panic!("fingerprint should not be read") };
        assert!(
            rebellions_native_with_fingerprints(925, &[], companion, unexpected_read).is_none()
        );
        assert!(rebellions_native_with_fingerprints(925, &[], || None, unexpected_read).is_none());
        assert!(rebellions_native_with_fingerprints(925, &[], runtime, || None).is_none());
        let startup =
            || Some("a25b9e9c8d4c7d8091b30d457fec070211fb86013e5d58dc6fdbec7ac0236ec1".to_owned());
        assert!(rebellions_native_with_fingerprints(925, &[], runtime, startup).is_none());
        for (index, arguments) in [(924, &[][..]), (926, &[][..]), (925, &[0; 4][..])] {
            assert!(
                rebellions_native_with_fingerprints(
                    index,
                    arguments,
                    unexpected_read,
                    unexpected_read
                )
                .is_none()
            );
        }
    }
}
