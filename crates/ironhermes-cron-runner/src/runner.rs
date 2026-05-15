// Placeholder — full implementation in Task 2 of Plan 32.1-06
use anyhow::Result;
use ironhermes_cron::CronJob;

pub struct CronRunnerContext;

pub async fn run_cron_job(_job: &CronJob, _ctx: &CronRunnerContext) -> Result<()> {
    unimplemented!("run_cron_job is implemented in Task 2")
}
