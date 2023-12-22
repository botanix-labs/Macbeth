# Botanix-k8s

Complete Helm charts for the entire Botanix stack

## Deploy Local Cluster with Minikube and Tilt

The following are prerequisites for spinning up the Botanix cluster locally:

- kubectl
`https://www.howtoforge.com/how-to-install-kubernetes-with-minikube-ubuntu-20-04/`

- Tilt:
`https://docs.tilt.dev/install.html`

- Helm:
`https://helm.sh/docs/intro/install/`

- minikube based on the following description:
`https://phoenixnap.com/kb/install-minikube-on-ubuntu`
`https://minikube.sigs.k8s.io/docs/start/`

 ...or alternatively use this tool which will automatically set up your cluster:
`https://github.com/tilt-dev/ctlptl##minikube-with-a-built-in-registry`

Execute in order:

1. Install minikube if needed by running `./minikub/install_minikube.sh`.
2. Run the following file `/minikube/run_minikube.sh` which should spin up the minikube cluster for you. Make sure there are no errors!
3. Run `kubectl create namespace botanix-local` to create a new namespace on the cluster called `botanix-local`
4. Run `kubectl config use-context minikube && kubectl config set-context --current --cluster=minikube --namespace=botanix-local` to set the current context to the latter namespace
5. Run `kubectl config get-contexts` to make sure your cluster is listed as `minikube` and the namespace `botanix-local` belongs to it
6. Run `kubectl -n botanix-local get pods` to make sure you see system pods running

## Using `k9s` for an interactive terminal UI

Install k9s from [here](https://github.com/derailed/k9s)

Run it with `k9s --context=<your kubectl context> --namespace=<namespace you want to watch>` e.g. `k9s --context=minikube --namespace=botanix-local`. You can do things like view logs with `l`, describe with `d`, delete with `Ctrl+d`.

## Useful links

* How [kubernetes works](https://www.youtube.com/watch?v=ZuIQurh_kDk)
* Kubernetes [concepts](https://kubernetes.io/docs/concepts/)
* Kubectl [overview](https://kubernetes.io/docs/reference/kubectl/overview/)
* Kubectl [cheat sheet](https://kubernetes.io/docs/reference/kubectl/cheatsheet/)
* Helm [chart tutorial](https://docs.bitnami.com/kubernetes/how-to/create-your-first-helm-chart/), then examine the helm charts in this repository, and the values yaml files that are used to template them. The defults values are in the charts themselves as `values.yaml`, and the values for specific configurations are at `values/<name>.yaml`.
* Tilt [tutorial](https://docs.tilt.dev/tutorial.html)


