{{define "main"}}
{{if .x}}
  {{template "known" .}}
{{else}}
  {{range .items}}
    {{template "missing" .}}
  {{end}}
{{end}}
{{block "blk" .}}
  {{template "\x69nner" .}}
{{end}}
{{end}}
{{define "known"}}K{{end}}
{{define "inner"}}I{{end}}
