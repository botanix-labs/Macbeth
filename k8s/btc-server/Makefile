.PHONY: deploy
deploy:
	kustomize build . | kubectl apply -f - -n sandbox