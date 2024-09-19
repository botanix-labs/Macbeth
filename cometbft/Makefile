.PHONY: build-cometbft-local start-cometbft-local stop-cometbft-local 

build-cometbft-local:
	docker-compose -f docker-compose-cometbft-local.yml up --build -d

start-cometbft-local:
	docker-compose -f docker-compose-cometbft-local.yml up -d

stop-cometbft-local:
	docker-compose -f docker-compose-cometbft-local.yml down

reset-cometbft-local-1:
	cometbft unsafe_reset_all --home ./cometbft-local-1

reset-cometbft-local-2:
	cometbft unsafe_reset_all --home ./cometbft-local-2
