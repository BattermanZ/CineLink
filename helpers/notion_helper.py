import os
import aiohttp
import asyncio
import json
import logging
from datetime import datetime
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# Notion API details
NOTION_API_KEY = os.getenv("NOTION_API_KEY")
DATABASE_ID = os.getenv("NOTION_DATABASE_ID")
NOTION_URL = "https://api.notion.com/v1/pages"
NOTION_QUERY_URL = f"https://api.notion.com/v1/databases/{DATABASE_ID}/query"

headers = {
    "Authorization": f"Bearer {NOTION_API_KEY}",
    "Content-Type": "application/json",
    "Notion-Version": "2022-06-28"
}

# Helper function to map a numeric rating (1 to 10) to emoji ratings
def numeric_to_emoji_rating(numeric_rating):
    rating_map = {
        1: "🌗", 2: "🌕", 3: "🌕🌗", 4: "🌕🌕", 5: "🌕🌕🌗",
        6: "🌕🌕🌕", 7: "🌕🌕🌕🌗", 8: "🌕🌕🌕🌕", 9: "🌕🌕🌕🌕🌗", 10: "🌕🌕🌕🌕🌕"
    }
    return rating_map.get(numeric_rating, "🌕")

# Asynchronous function to check if a movie exists in Notion
async def check_movie_exists(session, movie_title):
    logging.info(f"Checking if '{movie_title}' exists in Notion...")

    query_payload = {
        "filter": {
            "or": [
                {"property": "Name", "rich_text": {"contains": movie_title}},
                {"property": "Eng Name", "rich_text": {"contains": movie_title}}
            ]
        }
    }

    async with session.post(NOTION_QUERY_URL, headers=headers, json=query_payload) as response:
        if response.status == 200:
            result = await response.json()
            if result['results']:
                logging.info(f"Movie '{movie_title}' already exists in Notion.")
                return True
            else:
                logging.info(f"Movie '{movie_title}' does not exist in Notion.")
                return False
        else:
            logging.error(f"Failed to query Notion for '{movie_title}'. Status Code: {response.status}")
            return False

# Asynchronous function to add a movie to Notion
async def add_movie(session, movie):
    movie_title = movie['title']
    movie_rating = numeric_to_emoji_rating(movie['rating'])
    notion_movie_title = f"{movie_title};"

    # Get the current year to add to the "Years watched" property
    current_year = str(datetime.now().year)

    payload = {
        "parent": {"database_id": DATABASE_ID},
        "properties": {
            "Name": {
                "title": [
                    {"text": {"content": notion_movie_title}}  # Add semicolon only here for Notion entry
                ]
            },
            "Aurel's rating": {
                "select": {"name": movie_rating}
            },
            "Years watched": {
                "multi_select": [{"name": current_year}]  # Add the current year to the "Years watched" property
            }
        }
    }

    async with session.post(NOTION_URL, headers=headers, json=payload) as response:
        if response.status == 200:
            logging.info(f"Movie '{movie_title};' added to Notion with rating: '{movie_rating}', Year: '{current_year}'")
        else:
            response_text = await response.text()
            logging.error(f"Failed to add '{movie_title}' to Notion. Status Code: {response.status}, Response: {response_text}")

# Batch check if movies exist in Notion
async def batch_check_movies(movie_list):
    async with aiohttp.ClientSession() as session:
        tasks = [check_movie_exists(session, movie['title']) for movie in movie_list]
        results = await asyncio.gather(*tasks)
        return [movie_list[i]['title'] for i, exists in enumerate(results) if exists]

# Add movies to Notion in batches
async def add_movies_in_batch(movie_list):
    existing_movies = await batch_check_movies(movie_list)
    movies_to_add = [movie for movie in movie_list if movie['title'] not in existing_movies]

    if not movies_to_add:
        logging.info("No new movies to add to Notion.")
        return "No new movies to add."

    logging.info("Adding movies in batch to Notion...")

    async with aiohttp.ClientSession() as session:
        tasks = [add_movie(session, movie) for movie in movies_to_add]
        await asyncio.gather(*tasks)
    return "Batch processing completed."