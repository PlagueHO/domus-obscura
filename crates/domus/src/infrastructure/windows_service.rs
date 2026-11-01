//! SCM owns process supervision; the application still owns every durable effect.
use super::lifecycle::Lifecycle;
use crate::{Error, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::{define_windows_service, service_dispatcher};

pub const API_NAME: &str = "DomusObscuraApi";
pub const WORKER_NAME: &str = "DomusObscuraWorker";

struct Launch {
    role: Role,
    configuration: PathBuf,
    result: Mutex<Option<Result<()>>>,
}

#[derive(Clone, Copy)]
enum Role {
    Api,
    Worker,
}

impl Role {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "api" => Ok(Self::Api),
            "worker" => Ok(Self::Worker),
            _ => Err(Error::InvalidInput(
                "service role must be api or worker".into(),
            )),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Api => API_NAME,
            Self::Worker => WORKER_NAME,
        }
    }
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();
define_windows_service!(ffi_service_main, service_main);

pub fn configure_policy(role: &str, policy: &str) -> Result<()> {
    use windows_service::service::{
        ServiceAccess, ServiceAction, ServiceActionType, ServiceFailureActions,
        ServiceFailureResetPeriod,
    };
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let name = Role::parse(role)?.name();
    let enabled = match policy {
        "enable" => true,
        "disable" => false,
        _ => {
            return Err(Error::InvalidInput(
                "service policy must be enable or disable".into(),
            ));
        }
    };
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(native)?;
    let service = manager
        .open_service(
            name,
            ServiceAccess::QUERY_CONFIG | ServiceAccess::CHANGE_CONFIG | ServiceAccess::START,
        )
        .map_err(native)?;
    let configuration = service.query_config().map_err(native)?;
    let expected = format!(
        "\"{}\" windows-service {role} \"",
        std::env::current_exe()?.display()
    );
    if !configuration
        .executable_path
        .to_string_lossy()
        .to_ascii_lowercase()
        .starts_with(&expected.to_ascii_lowercase())
    {
        return Err(Error::Conflict(
            "service belongs to another installation".into(),
        ));
    }
    let mut actions = Vec::new();
    if enabled {
        for delay in [10, 30] {
            actions.push(ServiceAction {
                action_type: ServiceActionType::Restart,
                delay: Duration::from_secs(delay),
            });
        }
    }
    actions.push(ServiceAction {
        action_type: ServiceActionType::None,
        delay: Duration::ZERO,
    });
    service
        .update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(86400)),
            reboot_msg: None,
            command: None,
            actions: Some(actions),
        })
        .map_err(native)?;
    service
        .set_failure_actions_on_non_crash_failures(enabled)
        .map_err(native)?;
    service
        .set_preshutdown_timeout(Duration::from_secs(180))
        .map_err(native)?;
    Ok(())
}

fn native(error: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Io(std::io::Error::other(error))
}

struct Reporter {
    handle: ServiceStatusHandle,
    state: Mutex<ServiceState>,
}

impl Reporter {
    fn report(&self, next: ServiceState, exit: ServiceExitCode, checkpoint: u32) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| Error::Internal("service status lock failed"))?;
        if !transition_allowed(*state, next) {
            return Ok(());
        }
        self.handle
            .set_service_status(ServiceStatus {
                service_type: ServiceType::OWN_PROCESS,
                current_state: next,
                controls_accepted: if next == ServiceState::Running {
                    ServiceControlAccept::STOP
                        | ServiceControlAccept::PRESHUTDOWN
                        | ServiceControlAccept::SHUTDOWN
                } else {
                    ServiceControlAccept::empty()
                },
                exit_code: exit,
                checkpoint,
                wait_hint: if matches!(next, ServiceState::StartPending | ServiceState::StopPending)
                {
                    Duration::from_secs(180)
                } else {
                    Duration::ZERO
                },
                process_id: None,
            })
            .map_err(native)?;
        *state = next;
        Ok(())
    }
}

pub fn dispatch(role: &str, configuration: &Path) -> Result<()> {
    super::local_file::validate_reference(configuration)?;
    let role = Role::parse(role)?;
    LAUNCH
        .set(Launch {
            role,
            configuration: configuration.to_owned(),
            result: Mutex::new(None),
        })
        .map_err(|_| Error::Internal("service dispatcher was already initialized"))?;
    service_dispatcher::start(role.name(), ffi_service_main).map_err(native)?;
    LAUNCH
        .get()
        .ok_or(Error::Internal("service launch is missing"))?
        .result
        .lock()
        .map_err(|_| Error::Internal("service result lock failed"))?
        .take()
        .ok_or(Error::Internal("service exited without a result"))?
}

fn service_main(_arguments: Vec<OsString>) {
    let Some(launch) = LAUNCH.get() else { return };
    let result = run(launch);
    if result.is_err() {
        // Detailed inputs and credentials must not enter SCM diagnostics.
        eprintln!(
            "Windows service failed. Inspect configuration, permissions, and durable recovery state."
        );
    }
    match launch.result.lock() {
        Ok(mut saved) => *saved = Some(result),
        Err(_) => eprintln!("Windows service could not record its exit result."),
    }
}

fn run(launch: &Launch) -> Result<()> {
    let reporter_slot: Arc<OnceLock<Arc<Reporter>>> = Arc::new(OnceLock::new());
    let ready_slot = reporter_slot.clone();
    let lifecycle = Lifecycle::controlled(move || {
        ready_slot
            .get()
            .ok_or(Error::Internal("service status is not registered"))?
            .report(ServiceState::Running, ServiceExitCode::Win32(0), 0)
    });
    let stopping = lifecycle.clone();
    let controls = reporter_slot.clone();
    let handle =
        service_control_handler::register(launch.role.name(), move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown | ServiceControl::Preshutdown => {
                stopping.request_stop();
                match controls
                    .get()
                    .ok_or(Error::Internal("service status is not registered"))
                    .and_then(|reporter| {
                        reporter.report(ServiceState::StopPending, ServiceExitCode::Win32(0), 1)
                    }) {
                    Ok(()) => ServiceControlHandlerResult::NoError,
                    Err(_) => ServiceControlHandlerResult::Other(1),
                }
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        })
        .map_err(native)?;
    let reporter = Arc::new(Reporter {
        handle,
        state: Mutex::new(ServiceState::StartPending),
    });
    reporter_slot
        .set(reporter.clone())
        .map_err(|_| Error::Internal("service status registered twice"))?;
    reporter.report(ServiceState::StartPending, ServiceExitCode::Win32(0), 1)?;
    let result = (|| {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        match launch.role {
            Role::Api => {
                let prepared = super::server::PreparedServer::load_service(&launch.configuration)?;
                reporter.report(ServiceState::StartPending, ServiceExitCode::Win32(0), 2)?;
                runtime.block_on(prepared.run_with_lifecycle(lifecycle))
            }
            Role::Worker => {
                let prepared = super::production_worker::PreparedProductionWorker::load(
                    &launch.configuration,
                )?;
                reporter.report(ServiceState::StartPending, ServiceExitCode::Win32(0), 2)?;
                runtime.block_on(prepared.run_with_lifecycle(lifecycle))
            }
        }
    })();
    reporter.report(
        ServiceState::Stopped,
        if result.is_ok() {
            ServiceExitCode::Win32(0)
        } else {
            ServiceExitCode::ServiceSpecific(service_error_code(result.as_ref().err()))
        },
        0,
    )?;
    result
}

fn transition_allowed(current: ServiceState, next: ServiceState) -> bool {
    match current {
        ServiceState::Stopped => false,
        ServiceState::StopPending => {
            matches!(next, ServiceState::StopPending | ServiceState::Stopped)
        }
        ServiceState::Running => next != ServiceState::StartPending,
        _ => true,
    }
}

fn service_error_code(error: Option<&Error>) -> u32 {
    match error {
        None => 0,
        Some(Error::InvalidInput(_)) => 10,
        Some(Error::Io(_)) => 11,
        Some(Error::Database(_)) => 12,
        Some(Error::Authentication) => 13,
        Some(Error::Conflict(_)) => 14,
        Some(Error::Busy(_)) => 15,
        Some(Error::Remote(_) | Error::Certificate { .. }) => 16,
        Some(_) => 99,
    }
}

/// An API restart may reuse current schema, but cannot upgrade under a live worker.
pub(super) fn worker_stopped() -> Result<bool> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(native)?;
    match manager.open_service(WORKER_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(worker) => {
            Ok(worker.query_status().map_err(native)?.current_state == ServiceState::Stopped)
        }
        Err(windows_service::Error::Winapi(error)) if error.raw_os_error() == Some(1060) => {
            Ok(true)
        }
        Err(error) => Err(native(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_service_roles_share_exact_validation() -> Result<()> {
        assert_eq!(Role::parse("api")?.name(), API_NAME);
        assert_eq!(Role::parse("worker")?.name(), WORKER_NAME);
        for role in ["", "API", "worker ", "other"] {
            assert!(matches!(Role::parse(role), Err(Error::InvalidInput(_))));
            assert!(matches!(
                configure_policy(role, "disable"),
                Err(Error::InvalidInput(_))
            ));
        }
        Ok(())
    }

    #[test]
    fn windows_service_stop_cannot_be_reversed_by_startup_readiness() {
        assert!(!transition_allowed(
            ServiceState::StopPending,
            ServiceState::Running
        ));
        assert!(!transition_allowed(
            ServiceState::StopPending,
            ServiceState::StartPending
        ));
        assert!(transition_allowed(
            ServiceState::StopPending,
            ServiceState::Stopped
        ));
        assert!(!transition_allowed(
            ServiceState::Stopped,
            ServiceState::Running
        ));
    }

    #[test]
    fn windows_service_error_codes_are_redacted_and_not_success() {
        assert_eq!(service_error_code(None), 0);
        assert_eq!(
            service_error_code(Some(&Error::InvalidInput("private fixture".into()))),
            10
        );
        assert_eq!(service_error_code(Some(&Error::Authentication)), 13);
        assert_ne!(service_error_code(Some(&Error::Internal("fixture"))), 0);
    }
}
