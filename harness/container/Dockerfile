FROM python:3.14-slim
RUN apt-get update && apt-get install --no-install-recommends -y git && rm -rf /var/lib/apt/lists/*
RUN python -m pip install --no-cache-dir pytest==9.0.2
