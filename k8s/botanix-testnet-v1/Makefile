# Makefile for deploying, updating, and deleting the Helm chart

# Variables
CHART_NAME := botanix-testnet-v1
NAMESPACE := testnet

# Targets
deploy:
	helm upgrade --install $(CHART_NAME)-1 . --namespace $(NAMESPACE) --values testnet-1.yaml
	helm upgrade --install $(CHART_NAME)-2 . --namespace $(NAMESPACE) --values testnet-2.yaml
	helm upgrade --install $(CHART_NAME)-3 . --namespace $(NAMESPACE) --values non-fed-node-1.yaml
	kubectl apply -f ingressRoute.yaml -n $(NAMESPACE)
	kubectl apply -f configmap.yaml -n $(NAMESPACE)
	kubectl apply -f secret.yaml -n $(NAMESPACE)

update:
	helm upgrade $(CHART_NAME)-1 . --namespace $(NAMESPACE) --values testnet-1.yaml
	helm upgrade $(CHART_NAME)-2 . --namespace $(NAMESPACE) --values testnet-2.yaml
	helm upgrade $(CHART_NAME)-3 . --namespace $(NAMESPACE) --values non-fed-node-1.yaml
	kubectl apply -f ingressRoute.yaml -n $(NAMESPACE)
	kubectl apply -f configmap.yaml -n $(NAMESPACE)
	kubectl apply -f secret.yaml -n $(NAMESPACE)

delete:
	helm uninstall $(CHART_NAME)-1 --namespace $(NAMESPACE)
	helm uninstall $(CHART_NAME)-2 --namespace $(NAMESPACE)
	helm uninstall $(CHART_NAME)-3 --namespace $(NAMESPACE)
	kubectl delete -f ingressRoute.yaml -n $(NAMESPACE)
	kubectl delete -f configmap.yaml -n $(NAMESPACE)
	kubectl delete -f secret.yaml -n $(NAMESPACE)


