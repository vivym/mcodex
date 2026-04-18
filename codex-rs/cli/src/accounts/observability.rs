use anyhow::Result;
use anyhow::bail;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TargetPoolSource {
    CommandArg,
    TopLevelOverride,
    EffectivePool,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ResolvedTargetPool {
    pub pool_id: String,
    pub source: TargetPoolSource,
}

pub(crate) fn resolve_target_pool(
    command_pool: Option<&str>,
    top_level_override: Option<&str>,
    effective_pool_id: Option<&str>,
) -> Result<ResolvedTargetPool> {
    if let (Some(command_pool), Some(top_level_override)) = (command_pool, top_level_override)
        && command_pool != top_level_override
    {
        bail!("--pool `{command_pool}` conflicts with --account-pool `{top_level_override}`");
    }

    if let Some(command_pool) = command_pool {
        return Ok(ResolvedTargetPool {
            pool_id: command_pool.to_owned(),
            source: TargetPoolSource::CommandArg,
        });
    }

    if let Some(top_level_override) = top_level_override {
        return Ok(ResolvedTargetPool {
            pool_id: top_level_override.to_owned(),
            source: TargetPoolSource::TopLevelOverride,
        });
    }

    if let Some(effective_pool_id) = effective_pool_id {
        return Ok(ResolvedTargetPool {
            pool_id: effective_pool_id.to_owned(),
            source: TargetPoolSource::EffectivePool,
        });
    }

    bail!("no account pool is configured; pass --pool <POOL_ID> or configure a pool")
}

#[cfg(test)]
mod tests {
    use super::resolve_target_pool;

    #[test]
    fn resolve_target_pool_rejects_conflicting_command_and_override_pool_ids() {
        let err = resolve_target_pool(
            Some("team-command"),
            Some("team-override"),
            Some("team-effective"),
        )
        .expect_err("expected conflict");

        assert!(err.to_string().contains("conflicts with --account-pool"));
    }
}
