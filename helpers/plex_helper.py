import os
import logging
from plexapi.server import PlexServer
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# Plex server information
PLEX_URL = os.getenv("PLEX_URL")
PLEX_TOKEN = os.getenv("PLEX_TOKEN")

# Connect to Plex server
def connect_to_plex():
    try:
        logging.info(f"Connecting to Plex server at {PLEX_URL}")
        plex = PlexServer(PLEX_URL, PLEX_TOKEN)
        logging.info("Connected to Plex server successfully.")
        return plex
    except Exception as e:
        logging.error(f"Failed to connect to Plex server: {e}")
        return None

# Get all movies from Plex server
def get_all_movies(plex):
    try:
        # Retrieve all movies from the Plex library
        movies = plex.library.section('Movies').all()
        logging.info(f"Retrieved {len(movies)} movies from Plex.")
        return movies
    except Exception as e:
        logging.error(f"Failed to retrieve movies: {e}")
        return []