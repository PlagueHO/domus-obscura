use super::{Capability, ServiceConfiguration};
use crate::Result;
use crate::infrastructure::staging::StagingRoot;
use crate::infrastructure::store::{JobClaim, Store};

/// The implementation receives already-approved durable work, never an agent's
/// URL/path or credentials. Provider-specific submission still needs a journal.
pub trait AcquisitionProvider {
    fn id(&self) -> &str;
    fn acquire(
        &mut self,
        store: &mut Store,
        claim: &JobClaim,
        staging: &StagingRoot,
    ) -> Result<i64>;
}

pub fn acquire(
    configuration: &ServiceConfiguration,
    provider: &mut impl AcquisitionProvider,
    store: &mut Store,
    claim: &JobClaim,
    staging: &StagingRoot,
) -> Result<i64> {
    configuration.require(Capability::Acquisition, provider.id())?;
    store.job_work_boundary(claim)?;
    provider.acquire(store, claim, staging)
}
