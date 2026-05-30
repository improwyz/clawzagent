//! Convert between worker cron types and `clawz_services::dto`.

use clawz_services::dto::{
    CreateCronJobRequest as CreateCronJobDto, CronDeliveryDto, CronJobDto, CronRunResultDto,
};

use super::job::{CreateCronJobRequest, CronDelivery, CronJob, CronRunResult};

pub fn job_to_dto(job: CronJob) -> CronJobDto {
    CronJobDto {
        id: job.id,
        name: job.name,
        cron_expr: job.cron_expr,
        prompt: job.prompt,
        agent_id: job.agent_id,
        enabled: job.enabled,
        delivery: job.delivery.map(delivery_to_dto),
        disabled_toolsets: job.disabled_toolsets,
        last_run_at: job.last_run_at,
        created_at: job.created_at,
        updated_at: job.updated_at,
    }
}

fn delivery_to_dto(d: CronDelivery) -> CronDeliveryDto {
    CronDeliveryDto {
        channel_type: d.channel_type,
        config: d.config,
        to: d.to,
    }
}

fn delivery_from_dto(d: CronDeliveryDto) -> CronDelivery {
    CronDelivery {
        channel_type: d.channel_type,
        config: d.config,
        to: d.to,
    }
}

pub fn create_from_dto(req: CreateCronJobDto) -> CreateCronJobRequest {
    CreateCronJobRequest {
        cron_expr: req.cron_expr,
        prompt: req.prompt,
        agent_id: req.agent_id,
        name: req.name,
        enabled: req.enabled,
        delivery: req.delivery.map(delivery_from_dto),
        disabled_toolsets: req.disabled_toolsets,
    }
}

pub fn run_to_dto(r: CronRunResult) -> CronRunResultDto {
    CronRunResultDto {
        job_id: r.job_id,
        agent_id: r.agent_id,
        conversation_id: r.conversation_id,
        content: r.content,
        delivered: r.delivered,
    }
}
