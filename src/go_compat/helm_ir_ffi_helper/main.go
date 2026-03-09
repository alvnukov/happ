package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"sort"
	"strings"

	yamlv3 "go.yaml.in/yaml/v3"

	"helm.sh/helm/v3/pkg/chart/loader"
	"helm.sh/helm/v3/pkg/chartutil"
	"helm.sh/helm/v3/pkg/cli/values"
	"helm.sh/helm/v3/pkg/engine"
	"helm.sh/helm/v3/pkg/getter"
)

type request struct {
	ChartPath       string   `json:"chart_path"`
	ReleaseName     string   `json:"release_name"`
	Namespace       *string  `json:"namespace"`
	ValuesFiles     []string `json:"values_files"`
	SetValues       []string `json:"set_values"`
	SetStringValues []string `json:"set_string_values"`
	SetFileValues   []string `json:"set_file_values"`
	SetJSONValues   []string `json:"set_json_values"`
	KubeVersion     *string  `json:"kube_version"`
	APIVersions     []string `json:"api_versions"`
	IncludeCRDs     bool     `json:"include_crds"`
}

type response struct {
	OK          bool         `json:"ok"`
	Kind        string       `json:"kind,omitempty"`
	Error       string       `json:"error,omitempty"`
	Documents   []document   `json:"documents,omitempty"`
	Diagnostics []diagnostic `json:"diagnostics,omitempty"`
}

type document struct {
	Identity     identity `json:"identity"`
	Body         any      `json:"body"`
	TemplateFile *string  `json:"template_file,omitempty"`
	IncludeChain []string `json:"include_chain,omitempty"`
}

type identity struct {
	APIVersion *string `json:"api_version,omitempty"`
	Kind       *string `json:"kind,omitempty"`
	Name       *string `json:"name,omitempty"`
	Namespace  *string `json:"namespace,omitempty"`
}

type diagnostic struct {
	Severity      string `json:"severity"`
	Code          string `json:"code"`
	Message       string `json:"message"`
	DocumentIndex *int   `json:"document_index,omitempty"`
}

const maxRequestBytes = 32 * 1024 * 1024

func main() {
	in, err := io.ReadAll(io.LimitReader(os.Stdin, maxRequestBytes+1))
	if err != nil {
		writeError("decode", err)
		return
	}
	if len(in) > maxRequestBytes {
		writeError("decode", fmt.Errorf("request is too large (max %d bytes)", maxRequestBytes))
		return
	}

	var req request
	dec := json.NewDecoder(bytes.NewReader(in))
	dec.UseNumber()
	if err := dec.Decode(&req); err != nil {
		writeError("decode", fmt.Errorf("decode request: %w", err))
		return
	}

	docs, diags, err := buildResourceModel(&req)
	if err != nil {
		writeError("render", err)
		return
	}
	writeResponse(response{
		OK:          true,
		Documents:   docs,
		Diagnostics: diags,
	})
}

func buildResourceModel(req *request) ([]document, []diagnostic, error) {
	if strings.TrimSpace(req.ChartPath) == "" {
		return nil, nil, errors.New("chart path is empty")
	}

	ch, err := loader.Load(req.ChartPath)
	if err != nil {
		return nil, nil, fmt.Errorf("load chart: %w", err)
	}

	valueOpts := values.Options{
		ValueFiles:   req.ValuesFiles,
		Values:       req.SetValues,
		StringValues: req.SetStringValues,
		FileValues:   req.SetFileValues,
		JSONValues:   req.SetJSONValues,
	}
	mergedValues, err := valueOpts.MergeValues(getter.Providers{})
	if err != nil {
		return nil, nil, fmt.Errorf("merge values: %w", err)
	}

	caps := chartutil.DefaultCapabilities.Copy()
	if req.KubeVersion != nil && strings.TrimSpace(*req.KubeVersion) != "" {
		kv, err := chartutil.ParseKubeVersion(strings.TrimSpace(*req.KubeVersion))
		if err != nil {
			return nil, nil, fmt.Errorf("parse kube version %q: %w", *req.KubeVersion, err)
		}
		caps.KubeVersion = *kv
	}
	if len(req.APIVersions) > 0 {
		caps.APIVersions = append(caps.APIVersions, req.APIVersions...)
	}

	releaseName := strings.TrimSpace(req.ReleaseName)
	if releaseName == "" {
		releaseName = "imported"
	}
	namespace := "default"
	if req.Namespace != nil && strings.TrimSpace(*req.Namespace) != "" {
		namespace = strings.TrimSpace(*req.Namespace)
	}
	renderValues, err := chartutil.ToRenderValues(
		ch,
		mergedValues,
		chartutil.ReleaseOptions{
			Name:      releaseName,
			Namespace: namespace,
			Revision:  1,
			IsInstall: true,
			IsUpgrade: false,
		},
		caps,
	)
	if err != nil {
		return nil, nil, fmt.Errorf("build render values: %w", err)
	}

	rendered, err := engine.Render(ch, renderValues)
	if err != nil {
		return nil, nil, err
	}

	paths := make([]string, 0, len(rendered))
	for path := range rendered {
		paths = append(paths, path)
	}
	sort.Strings(paths)

	allDocs := make([]document, 0, len(paths)*2)
	diags := make([]diagnostic, 0)
	for _, path := range paths {
		docs, err := decodeDocumentsFromYAML(rendered[path], path)
		if err != nil {
			return nil, nil, fmt.Errorf("decode rendered template %s: %w", path, err)
		}
		allDocs = append(allDocs, docs...)
	}

	if req.IncludeCRDs {
		for _, crd := range ch.CRDObjects() {
			docs, err := decodeDocumentsFromYAML(string(crd.File.Data), crd.Filename)
			if err != nil {
				return nil, nil, fmt.Errorf("decode rendered CRD %s: %w", crd.Filename, err)
			}
			allDocs = append(allDocs, docs...)
		}
	}

	return allDocs, diags, nil
}

func decodeDocumentsFromYAML(stream string, templateFile string) ([]document, error) {
	dec := yamlv3.NewDecoder(strings.NewReader(stream))
	out := make([]document, 0, 2)
	for {
		var raw any
		err := dec.Decode(&raw)
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return nil, err
		}
		if raw == nil {
			continue
		}
		raw = normalizeYAMLValue(raw)
		obj, ok := raw.(map[string]any)
		if !ok {
			continue
		}
		apiVersion, _ := obj["apiVersion"].(string)
		kind, _ := obj["kind"].(string)
		if strings.TrimSpace(apiVersion) == "" || strings.TrimSpace(kind) == "" {
			continue
		}
		doc := document{
			Identity: identity{
				APIVersion: toStringPtr(apiVersion),
				Kind:       toStringPtr(kind),
				Name:       extractMetadataString(obj, "name"),
				Namespace:  extractMetadataString(obj, "namespace"),
			},
			Body:         obj,
			TemplateFile: toStringPtr(templateFile),
			IncludeChain: []string{},
		}
		out = append(out, doc)
	}
	return out, nil
}

func extractMetadataString(obj map[string]any, key string) *string {
	md, ok := obj["metadata"].(map[string]any)
	if !ok {
		return nil
	}
	v, ok := md[key].(string)
	if !ok || strings.TrimSpace(v) == "" {
		return nil
	}
	return &v
}

func normalizeYAMLValue(v any) any {
	switch value := v.(type) {
	case map[string]any:
		out := make(map[string]any, len(value))
		for k, vv := range value {
			out[k] = normalizeYAMLValue(vv)
		}
		return out
	case map[any]any:
		out := make(map[string]any, len(value))
		for k, vv := range value {
			out[fmt.Sprint(k)] = normalizeYAMLValue(vv)
		}
		return out
	case []any:
		out := make([]any, 0, len(value))
		for _, item := range value {
			out = append(out, normalizeYAMLValue(item))
		}
		return out
	default:
		return value
	}
}

func toStringPtr(v string) *string {
	if strings.TrimSpace(v) == "" {
		return nil
	}
	return &v
}

func writeError(kind string, err error) {
	writeResponse(response{
		OK:    false,
		Kind:  kind,
		Error: err.Error(),
	})
}

func writeResponse(resp response) {
	enc := json.NewEncoder(os.Stdout)
	_ = enc.Encode(resp)
}
