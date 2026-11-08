#!/bin/bash
docker compose -f ./docker-compose.v2.yml down -v

docker rm -vf $(docker ps -aq)
#docker rmi -f $(docker images -aq)

sudo rm -rf ./data_v2 ./logs_dump ./startup_v2 ./metrics_server ./log_processor.out ./docker-compose.v2.yml
