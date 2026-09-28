#!/usr/bin/env -S uv run
# /// script
# requires-python = ">=3.11"
# dependencies = [
#     "requests",
# ]
# ///

import requests

def main():
    url = "https://www.google.com"
    
    print(f"Fetching {url}...")
    response = requests.get(url)
    
    print(f"Status Code: {response.status_code}")
    print(f"Content Length: {len(response.text)} characters")
    print("\nFirst 500 characters:")
    print(response.text[:500])

if __name__ == "__main__":
    main()