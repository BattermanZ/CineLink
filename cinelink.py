from flask import Flask, render_template, redirect, url_for, request
from flask_socketio import SocketIO
from helpers.notion_helper import add_movies_in_batch
from helpers.plex_helper import connect_to_plex, get_all_movies
from helpers.log_setup import setup_logging
from helpers.scheduler_helper import schedule_task
from datetime import datetime
import logging
import re
import time
import os
import asyncio
from dotenv import load_dotenv
import traceback
import subprocess
from gevent import monkey
from geventwebsocket.handler import WebSocketHandler

# Load environment variables from .env file
load_dotenv()

# Set up logging
setup_logging()

# Apply monkey patching for gevent
monkey.patch_all()

# Initialize Flask app
app = Flask(__name__)
socketio = SocketIO(app, async_mode='gevent', logger=True, engineio_logger=True)

# Global variable to store the currently scheduled time
scheduled_time = None

# Function to parse logs and extract information for the last movie and the last 8 movies
def parse_logs():
    last_movie = None
    last_8_movies = []
    last_run_time = None

    # Regular expression pattern to match movies added to Notion
    movie_addition_pattern = re.compile(r"Movie '(.+?);' added to Notion with rating: '(.+?)'")

    with open('logs/logs.txt', 'r') as f:
        lines = f.readlines()

    # Reverse the log lines to show them anti-chronologically
    lines.reverse()

    found_movies = []

    for line in lines:
        # Extract the script's last run time from the logs
        if "Script finished at" in line and not last_run_time:
            last_run_time = line.split("Script finished at ")[-1].strip()
            last_run_time = re.split(r'[.,]', last_run_time)[0]  # Strip out microseconds

        # Match log entries for movie additions
        match = movie_addition_pattern.search(line)
        if match:
            movie_title = match.group(1)
            movie_rating = match.group(2)
            found_movies.append((movie_title, movie_rating))

    # Set the last movie added as the most recent match
    if found_movies:
        last_movie = found_movies[0]  # The first match is the most recent

    # Get the last 8 movies added excluding the last movie
    if len(found_movies) > 1:
        last_8_movies = found_movies[1:9]  # Exclude the first movie (last movie), then take the next 8

    return last_movie, last_8_movies, last_run_time or "No script run yet", lines

# Route to set the schedule dynamically
@app.route('/set-schedule', methods=['POST'])
def set_schedule():
    global scheduled_time
    time_str = request.form['time']
    hour, minute = map(int, time_str.split(':'))

    schedule_task(run_script, hour, minute)

    scheduled_time = time_str
    logging.info(f"Automation time set to {time_str}")

    return redirect(url_for('home'))

# Function to run the script and push updates
def run_script():
    socketio.emit('log_update', {'log_entry': "Starting script..."})
    logging.info("Starting script...")

    plex = connect_to_plex()
    if not plex:
        log_message = "Failed to connect to Plex server."
        logging.error(log_message)
        socketio.emit('log_update', {'log_entry': log_message})
        return

    movies = get_all_movies(plex)
    log_message = f"Found {len(movies)} movies in your Plex library."
    logging.info(log_message)
    socketio.emit('log_update', {'log_entry': log_message})

    # Simulate some processing delay for demonstration purposes
    time.sleep(2)

    # Gather movies to be processed
    movie_list = []
    for movie in movies:
        user_rating = movie.userRating
        if user_rating is not None:
            title = movie.title
            log_message = f"Processing: {title} | Plex Rating: {user_rating}"
            logging.info(log_message)
            socketio.emit('log_update', {'log_entry': log_message})
            time.sleep(0.1)  # Small delay to ensure the UI updates
            movie_list.append({"title": title, "rating": user_rating})

    # Call the batch adding function for Notion
    try:
        logging.info("Attempting to add movies to Notion...")
        socketio.emit('log_update', {'log_entry': "Attempting to add movies to Notion..."})
        logging.info(f"Payload being sent: {movie_list}")
        loop = asyncio.new_event_loop()
        asyncio.set_event_loop(loop)
        response = loop.run_until_complete(add_movies_in_batch(movie_list))
        logging.info(f"Notion API Response: {response}")
        socketio.emit('log_update', {'log_entry': f"Notion API Response: {response}"})
        logging.info("Movies successfully added to Notion.")
        socketio.emit('log_update', {'log_entry': "Movies successfully added to Notion."})
    except Exception as e:
        error_message = f"Failed to add movies to Notion: {str(e)}"
        logging.error(error_message)
        logging.error(traceback.format_exc())
        socketio.emit('log_update', {'log_entry': error_message})

    final_message = f"Script finished at {datetime.now()}"
    logging.info(final_message)
    socketio.emit('log_update', {'log_entry': final_message})

# Route to manually run the script
@app.route('/run-script-stream')
def run_script_stream():
    socketio.start_background_task(run_script)
    return redirect(url_for('home'))

# Route to display all logs
@app.route('/')
def home():
    last_movie, last_8_movies, last_run_time, logs = parse_logs()
    return render_template('gui.html',
                           last_movie=last_movie,
                           last_8_movies=last_8_movies,
                           last_run_time=last_run_time,
                           logs="".join(logs),
                           scheduled_time=scheduled_time)

if __name__ == "__main__":
    subprocess.run(["gunicorn", "-w", "1", "-k", "geventwebsocket.gunicorn.workers.GeventWebSocketWorker", "-b", "0.0.0.0:3146", "cinelink:app"])