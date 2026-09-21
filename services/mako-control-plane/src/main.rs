//! Mako Cloud control-plane service entrypoint.

#![forbid(unsafe_code)]

use std::{
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use futures::executor::block_on;
use mako_config::{ServiceConfig, ServiceKind};
use mako_control_plane::ApplicationMailError;
use mako_control_plane_service::{ControlPlaneGraph, control_plane_router};
use mako_service_runtime::{
    HttpTransportConfig, ReadinessProbe, serve_http_transport_with_readiness,
};

fn main() -> ExitCode {
    start(ServiceKind::ControlPlane)
}

const fn application_mail_failure_class(error: &ApplicationMailError) -> &'static str {
    match error {
        ApplicationMailError::Source(_) => "data_plane",
        ApplicationMailError::Storage(_) | ApplicationMailError::LimitExceeded => "storage",
        _ => "delivery_worker",
    }
}

fn start(service: ServiceKind) -> ExitCode {
    let config = match ServiceConfig::load_from_process(service) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let graph = match ControlPlaneGraph::open(&config) {
        Ok(graph) => Arc::new(graph),
        Err(error) => {
            eprintln!("control-plane graph not ready: {error}");
            return ExitCode::FAILURE;
        }
    };
    let readiness = graph.readiness();
    println!(
        "startup configuration valid: {}",
        config.startup_diagnostic()
    );
    println!("{readiness}");

    let mut transport = HttpTransportConfig::new(
        config.bind_address,
        service.name(),
        "control_sqlite_auth_rbac_audit_management_ready",
    );
    transport.max_request_body_bytes = usize::try_from(config.max_request_bytes)
        .expect("validated request size must fit this platform");
    transport.shutdown_grace = config.shutdown_grace;
    let readiness_probe: Arc<dyn ReadinessProbe> = graph.clone();
    let router = match control_plane_router(Arc::clone(&graph)) {
        Ok(router) => router,
        Err(error) => {
            eprintln!("control-plane route composition failed: {error}");
            return ExitCode::FAILURE;
        }
    };
    let mail_stopping = Arc::new(AtomicBool::new(false));
    let mail_worker = graph.developer_mail_worker().cloned().map(|worker| {
        let stopping = Arc::clone(&mail_stopping);
        let metrics = Arc::clone(&graph);
        // Application mail rides the same thread and transport: first the
        // developer outbox, then a drain of the data plane's intents into the
        // application outbox and a delivery pass over it.
        let application_worker = graph.application_mail_worker().cloned();
        thread::spawn(move || {
            let mut application_failures: u64 = 0;
            while !stopping.load(Ordering::Acquire) {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs());
                match now {
                    Ok(now) => {
                        match block_on(worker.run_once(now)) {
                            Ok(report) => metrics.observe_developer_mail(&report),
                            Err(_) => {
                                metrics.observe_developer_mail_worker_failure();
                                eprintln!(
                                    "developer mail outbox pass failed: class=delivery_worker"
                                );
                            }
                        }
                        if let Some(application) = &application_worker {
                            match block_on(application.run_once(now)) {
                                Ok(report) => {
                                    application_failures = 0;
                                    metrics.observe_application_mail(&report);
                                }
                                Err(error) => {
                                    metrics.observe_application_mail_worker_failure();
                                    // The data plane may simply not be up yet;
                                    // say so once, then once a minute.
                                    if application_failures.is_multiple_of(12) {
                                        // The class alone says which half of the
                                        // platform refused, which is not enough to
                                        // act on: carry the error too.
                                        eprintln!(
                                            "application mail pass failed: class={} detail={error}",
                                            application_mail_failure_class(&error)
                                        );
                                    }
                                    application_failures = application_failures.saturating_add(1);
                                }
                            }
                        }
                    }
                    Err(_) => eprintln!("developer mail outbox pass failed: class=clock"),
                }
                thread::park_timeout(Duration::from_secs(5));
            }
        })
    });
    let operator_maintenance = {
        let stopping = Arc::clone(&mail_stopping);
        let graph = Arc::clone(&graph);
        thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                {
                    Ok(now) => {
                        if block_on(graph.cleanup_operator_authentication(now)).is_err() {
                            eprintln!("operator authentication maintenance failed: class=storage");
                        }
                    }
                    Err(_) => {
                        eprintln!("operator authentication maintenance failed: class=clock");
                    }
                }
                thread::park_timeout(Duration::from_secs(60));
            }
        })
    };
    let data_job_worker = {
        let stopping = Arc::clone(&mail_stopping);
        let worker = graph.data_job_service().clone();
        thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                {
                    Ok(now) => {
                        if block_on(worker.run_worker_once(now)).is_err() {
                            eprintln!("data-job worker pass failed: class=worker_dependency");
                        }
                    }
                    Err(_) => eprintln!("data-job worker pass failed: class=clock"),
                }
                thread::park_timeout(Duration::from_secs(2));
            }
        })
    };
    // Webhooks: consume each environment's change feed into the outbox and
    // deliver what is due. Every two seconds, like the data-job worker, so a
    // change reaches its endpoint promptly without contending for the
    // single-writer control store.
    let webhook_worker = {
        let stopping = Arc::clone(&mail_stopping);
        let graph = Arc::clone(&graph);
        thread::spawn(move || {
            let mut failures: u64 = 0;
            let mut intake_failures: u64 = 0;
            while !stopping.load(Ordering::Acquire) {
                match SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                {
                    Ok(now) => match block_on(graph.webhook_worker().run_once(now)) {
                        Ok(report) => {
                            failures = 0;
                            graph.observe_webhooks(&report);
                            if report.intake_failures == 0 {
                                intake_failures = 0;
                            } else {
                                // The data plane may be down or a subscribed
                                // collection gone; say so once, then once a minute.
                                if intake_failures.is_multiple_of(30) {
                                    eprintln!(
                                        "webhook intake could not read every change feed: class=data_plane count={}",
                                        report.intake_failures
                                    );
                                }
                                intake_failures = intake_failures.saturating_add(1);
                            }
                        }
                        Err(_) => {
                            graph.observe_webhook_worker_failure();
                            if failures.is_multiple_of(30) {
                                eprintln!("webhook worker pass failed: class=storage");
                            }
                            failures = failures.saturating_add(1);
                        }
                    },
                    Err(_) => eprintln!("webhook worker pass failed: class=clock"),
                }
                thread::park_timeout(Duration::from_secs(2));
            }
        })
    };
    // Scheduled functions: every five seconds, fire what has fallen due
    // through the edge gateway and record the run. A pass runs its
    // invocations one after another, so the cadence is the floor on how
    // promptly a due time is noticed, not a bound on how long a pass takes.
    let function_schedule_worker = {
        let stopping = Arc::clone(&mail_stopping);
        let graph = Arc::clone(&graph);
        thread::spawn(move || {
            let mut failures: u64 = 0;
            while !stopping.load(Ordering::Acquire) {
                match SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                {
                    Ok(now) => match block_on(graph.function_schedule_worker().run_once(now)) {
                        Ok(report) => {
                            failures = 0;
                            graph.observe_function_schedules(&report);
                        }
                        Err(_) => {
                            graph.observe_function_schedule_worker_failure();
                            if failures.is_multiple_of(12) {
                                eprintln!("function schedule worker pass failed: class=storage");
                            }
                            failures = failures.saturating_add(1);
                        }
                    },
                    Err(_) => eprintln!("function schedule worker pass failed: class=clock"),
                }
                thread::park_timeout(Duration::from_secs(5));
            }
        })
    };
    // Custom domains: once a minute, look each domain's verification record
    // up and publish every environment's verified list. A pass looks the
    // records up one after another, so the cadence is a floor.
    let custom_domain_worker = {
        let stopping = Arc::clone(&mail_stopping);
        let graph = Arc::clone(&graph);
        thread::spawn(move || {
            let mut failures: u64 = 0;
            while !stopping.load(Ordering::Acquire) {
                match SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                {
                    Ok(now) => match block_on(graph.custom_domain_verifier().run_once(now)) {
                        Ok(report) => {
                            failures = 0;
                            graph.observe_custom_domains(&report);
                            if report.publish_failures > 0 {
                                eprintln!(
                                    "custom domain list could not be published: class=data_plane count={}",
                                    report.publish_failures
                                );
                            }
                        }
                        Err(_) => {
                            graph.observe_custom_domain_worker_failure();
                            if failures.is_multiple_of(5) {
                                eprintln!("custom domain verifier pass failed: class=storage");
                            }
                            failures = failures.saturating_add(1);
                        }
                    },
                    Err(_) => eprintln!("custom domain verifier pass failed: class=clock"),
                }
                thread::park_timeout(Duration::from_secs(60));
            }
        })
    };
    // Project and environment creation enqueue provisioning and report an
    // asynchronous state. Without this pass those resources stay in
    // `provisioning` and never expose a usable data plane.
    let provisioning_worker = {
        let stopping = Arc::clone(&mail_stopping);
        let graph = Arc::clone(&graph);
        thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                {
                    Ok(now) => {
                        let (_advanced, failed) = block_on(graph.run_pending_provisioning(now));
                        if failed > 0 {
                            eprintln!(
                                "provisioning worker pass advanced with failures: class=provisioning count={failed}"
                            );
                        }
                    }
                    Err(_) => eprintln!("provisioning worker pass failed: class=clock"),
                }
                // Control storage is a single-writer SQLite database shared with
                // request handling and the readiness probe. A tight loop here
                // starves them, so this polls at a cadence that still advances
                // provisioning promptly without contending for the write lock.
                thread::park_timeout(Duration::from_secs(10));
            }
        })
    };
    // Let project and environment creation unpark the worker so a new resource
    // provisions at once instead of waiting for the next idle poll.
    mako_control_plane_service::PROVISIONING_WAKE
        .set(provisioning_worker.thread().clone())
        .ok();
    // Function logs live in the runtime supervisor's bounded in-memory
    // buffer until this pass carries them into the retained telemetry store,
    // scrubbed. Off the request path like every other measurement.
    let function_log_worker = {
        let stopping = Arc::clone(&mail_stopping);
        let graph = Arc::clone(&graph);
        thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let now_milliseconds = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
                    .unwrap_or(0);
                block_on(graph.collect_function_logs(now_milliseconds));
                while graph.telemetry_emitter().flush_once() > 0 {}
                thread::park_timeout(Duration::from_secs(15));
            }
            // Ship what is already buffered before the process exits.
            while graph.telemetry_emitter().flush_once() > 0 {}
        })
    };
    let runtime = serve_http_transport_with_readiness(transport, router, readiness_probe);
    mail_stopping.store(true, Ordering::Release);
    operator_maintenance.thread().unpark();
    data_job_worker.thread().unpark();
    webhook_worker.thread().unpark();
    function_schedule_worker.thread().unpark();
    custom_domain_worker.thread().unpark();
    provisioning_worker.thread().unpark();
    function_log_worker.thread().unpark();
    if let Some(worker) = &mail_worker {
        worker.thread().unpark();
    }
    let mail_shutdown = mail_worker
        .map(|worker| {
            worker
                .join()
                .map_err(|_| "developer mail worker did not stop")
        })
        .transpose();
    let operator_maintenance_shutdown = operator_maintenance
        .join()
        .map_err(|_| "operator authentication maintenance worker did not stop");
    let data_job_worker_shutdown = data_job_worker
        .join()
        .map_err(|_| "data-job worker did not stop")
        .and_then(|()| {
            webhook_worker
                .join()
                .map_err(|_| "webhook worker did not stop")
        })
        .and_then(|()| {
            function_schedule_worker
                .join()
                .map_err(|_| "function schedule worker did not stop")
        })
        .and_then(|()| {
            custom_domain_worker
                .join()
                .map_err(|_| "custom domain worker did not stop")
        });
    let provisioning_worker_shutdown = provisioning_worker
        .join()
        .map_err(|_| "provisioning worker did not stop");
    let function_log_worker_shutdown = function_log_worker
        .join()
        .map_err(|_| "function log worker did not stop");
    let shutdown = Arc::try_unwrap(graph)
        .map_err(|_| "control-plane graph still has active owners")
        .and_then(|graph| {
            block_on(graph.shutdown()).map_err(|_| "control-plane graph shutdown did not complete")
        });
    match (
        runtime,
        shutdown,
        mail_shutdown,
        operator_maintenance_shutdown,
        data_job_worker_shutdown,
        provisioning_worker_shutdown,
        function_log_worker_shutdown,
    ) {
        (Ok(()), Ok(()), Ok(_), Ok(()), Ok(()), Ok(()), Ok(())) => ExitCode::SUCCESS,
        (Err(error), _, _, _, _, _, _) => {
            eprintln!("service runtime failed: {error}");
            ExitCode::FAILURE
        }
        (_, Err(error), _, _, _, _, _) => {
            eprintln!("service shutdown failed: {error}");
            ExitCode::FAILURE
        }
        (_, _, Err(error), _, _, _, _)
        | (_, _, _, Err(error), _, _, _)
        | (_, _, _, _, Err(error), _, _)
        | (_, _, _, _, _, Err(error), _)
        | (_, _, _, _, _, _, Err(error)) => {
            eprintln!("service shutdown failed: {error}");
            ExitCode::FAILURE
        }
    }
}
