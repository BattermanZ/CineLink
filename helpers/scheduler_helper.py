from apscheduler.schedulers.background import BackgroundScheduler

scheduler = BackgroundScheduler()

def schedule_task(run_script, hour, minute):
    scheduler.remove_all_jobs()
    scheduler.add_job(run_script, trigger='cron', hour=hour, minute=minute)
    scheduler.start()