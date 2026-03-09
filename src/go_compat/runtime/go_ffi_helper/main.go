package main

// Go parity reference: go/src/text/template/*.go.

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"math"
	"os"
	"reflect"
	"strconv"
	"strings"
	"text/template"
)

type request struct {
	Template   string      `json:"template"`
	Data       interface{} `json:"data"`
	MissingKey string      `json:"missing_key"`
	Functions  []string    `json:"functions"`
}

type response struct {
	OK     bool   `json:"ok"`
	Output string `json:"output,omitempty"`
	Error  string `json:"error,omitempty"`
	Kind   string `json:"kind,omitempty"`
}

var interfaceType = reflect.TypeOf((*interface{})(nil)).Elem()

const maxRequestBytes = 8 * 1024 * 1024

func main() {
	input, err := io.ReadAll(io.LimitReader(os.Stdin, maxRequestBytes+1))
	if err != nil {
		writeErr("io", err)
		return
	}
	if len(input) > maxRequestBytes {
		writeErr("decode", fmt.Errorf("request is too large (max %d bytes)", maxRequestBytes))
		return
	}

	var req request
	dec := json.NewDecoder(bytes.NewReader(input))
	dec.UseNumber()
	if err := dec.Decode(&req); err != nil {
		writeErr("decode", err)
		return
	}

	data := decodeValue(req.Data)
	tpl := template.New("happ-go-ffi")
	if req.MissingKey != "" {
		tpl = tpl.Option("missingkey=" + req.MissingKey)
	}
	if fm := stubFuncMap(req.Functions); len(fm) > 0 {
		tpl = tpl.Funcs(fm)
	}

	parsed, err := tpl.Parse(normalizeTemplate(req.Template))
	if err != nil {
		writeErr("parse", err)
		return
	}

	var out bytes.Buffer
	if err := parsed.Execute(&out, data); err != nil {
		writeErr("execute", err)
		return
	}

	writeResponse(response{
		OK:     true,
		Output: out.String(),
	})
}

func writeErr(kind string, err error) {
	writeResponse(response{
		OK:    false,
		Error: err.Error(),
		Kind:  kind,
	})
}

func writeResponse(resp response) {
	enc := json.NewEncoder(os.Stdout)
	_ = enc.Encode(resp)
}

func stubFuncMap(names []string) template.FuncMap {
	out := template.FuncMap{}
	for _, raw := range names {
		name := strings.TrimSpace(raw)
		if name == "" || isBuiltinFunctionName(name) {
			continue
		}
		n := name
		out[n] = func(args ...interface{}) (interface{}, error) {
			return nil, fmt.Errorf("\"%s\" is not a defined function", n)
		}
	}
	return out
}

func isBuiltinFunctionName(name string) bool {
	switch name {
	case "and",
		"call",
		"or",
		"not",
		"len",
		"index",
		"slice",
		"html",
		"js",
		"print",
		"printf",
		"println",
		"urlquery",
		"eq",
		"ne",
		"lt",
		"le",
		"gt",
		"ge":
		return true
	default:
		return false
	}
}

func normalizeTemplate(src string) string {
	if !strings.Contains(src, "{{") {
		return src
	}
	var out strings.Builder
	out.Grow(len(src) + 16)
	cursor := 0
	for cursor < len(src) {
		openRel := strings.Index(src[cursor:], "{{")
		if openRel < 0 {
			out.WriteString(src[cursor:])
			break
		}
		open := cursor + openRel
		out.WriteString(src[cursor:open])

		actionStart := open + 2
		closeRel := strings.Index(src[actionStart:], "}}")
		if closeRel < 0 {
			out.WriteString(src[open:])
			break
		}
		actionEnd := actionStart + closeRel
		out.WriteString("{{")
		out.WriteString(normalizeActionInner(src[actionStart:actionEnd]))
		out.WriteString("}}")
		cursor = actionEnd + 2
	}
	return out.String()
}

func normalizeActionInner(inner string) string {
	const (
		stateNormal = iota
		stateSingle
		stateDouble
		stateRaw
	)
	state := stateNormal
	out := make([]byte, 0, len(inner)+8)

	for i := 0; i < len(inner); i++ {
		ch := inner[i]
		switch state {
		case stateSingle:
			out = append(out, ch)
			if ch == '\\' && i+1 < len(inner) {
				i++
				out = append(out, inner[i])
				continue
			}
			if ch == '\'' {
				state = stateNormal
			}
			continue
		case stateDouble:
			out = append(out, ch)
			if ch == '\\' && i+1 < len(inner) {
				i++
				out = append(out, inner[i])
				continue
			}
			if ch == '"' {
				state = stateNormal
			}
			continue
		case stateRaw:
			out = append(out, ch)
			if ch == '`' {
				state = stateNormal
			}
			continue
		}

		switch ch {
		case '\'':
			state = stateSingle
			out = append(out, ch)
			continue
		case '"':
			state = stateDouble
			out = append(out, ch)
			continue
		case '`':
			state = stateRaw
			out = append(out, ch)
			continue
		}

		if i+1 < len(inner) && ch == ':' && inner[i+1] == '=' {
			appendSpaceBefore(&out)
			out = append(out, ':', '=')
			i++
			appendSpaceAfter(&out, inner, i+1)
			continue
		}

		if ch == '=' && shouldNormalizeAssign(inner, i, out) {
			appendSpaceBefore(&out)
			out = append(out, '=')
			appendSpaceAfter(&out, inner, i+1)
			continue
		}

		out = append(out, ch)
	}
	return string(out)
}

func shouldNormalizeAssign(inner string, index int, current []byte) bool {
	if index+1 < len(inner) && inner[index+1] == '=' {
		return false
	}
	prev := lastNonSpace(current)
	next := nextNonSpace(inner, index+1)
	if prev == 0 || next == 0 {
		return false
	}
	if prev == ':' || prev == '<' || prev == '>' || prev == '!' || prev == '=' {
		return false
	}
	if next == '=' {
		return false
	}
	return true
}

func appendSpaceBefore(dst *[]byte) {
	if len(*dst) == 0 {
		return
	}
	last := (*dst)[len(*dst)-1]
	if !isSpace(last) {
		*dst = append(*dst, ' ')
	}
}

func appendSpaceAfter(dst *[]byte, src string, index int) {
	next := nextNonSpace(src, index)
	if next == 0 {
		return
	}
	if !isSpace(src[index]) {
		*dst = append(*dst, ' ')
	}
}

func lastNonSpace(buf []byte) byte {
	for i := len(buf) - 1; i >= 0; i-- {
		if !isSpace(buf[i]) {
			return buf[i]
		}
	}
	return 0
}

func nextNonSpace(src string, start int) byte {
	for i := start; i < len(src); i++ {
		if !isSpace(src[i]) {
			return src[i]
		}
	}
	return 0
}

func isSpace(b byte) bool {
	return b == ' ' || b == '\t' || b == '\n' || b == '\r'
}

func decodeValue(raw interface{}) interface{} {
	switch value := raw.(type) {
	case nil:
		return nil
	case json.Number:
		if i, err := value.Int64(); err == nil {
			return int(i)
		}
		if u, err := strconv.ParseUint(value.String(), 10, 64); err == nil {
			return int(u)
		}
		if f, err := value.Float64(); err == nil {
			return f
		}
		return value.String()
	case float64:
		if isWholeFloat(value) {
			return int(value)
		}
		return value
	case map[string]interface{}:
		if typeName, ok := value["__happ_go_type"].(string); ok {
			payload := value["__happ_go_value"]
			return decodeTypedValue(typeName, payload)
		}
		out := make(map[string]interface{}, len(value))
		for key, item := range value {
			out[key] = decodeValue(item)
		}
		return out
	case []interface{}:
		out := make([]interface{}, len(value))
		for i, item := range value {
			out[i] = decodeValue(item)
		}
		return out
	default:
		return value
	}
}

func decodeTypedValue(typeName string, payload interface{}) interface{} {
	switch typeName {
	case "[]byte":
		return decodeByteSlice(payload)
	case "string-bytes":
		bytes := decodeByteSlice(payload)
		return string(bytes)
	}

	if strings.HasPrefix(typeName, "map[string]") {
		return decodeTypedMap(typeName[len("map[string]"):], payload)
	}
	if strings.HasPrefix(typeName, "[]") {
		return decodeTypedSlice(typeName[2:], payload)
	}
	return decodeValue(payload)
}

func decodeTypedMap(elemType string, payload interface{}) interface{} {
	mapType, ok := goTypeFromName("map[string]" + elemType)
	if !ok {
		return decodeValue(payload)
	}
	if payload == nil {
		return reflect.Zero(mapType).Interface()
	}
	rawMap, ok := payload.(map[string]interface{})
	if !ok {
		return reflect.Zero(mapType).Interface()
	}
	typed := reflect.MakeMapWithSize(mapType, len(rawMap))
	elem := mapType.Elem()
	for key, raw := range rawMap {
		converted, ok := convertToType(raw, elem)
		if !ok {
			converted = reflect.Zero(elem)
		}
		typed.SetMapIndex(reflect.ValueOf(key), converted)
	}
	return typed.Interface()
}

func decodeTypedSlice(elemType string, payload interface{}) interface{} {
	sliceType, ok := goTypeFromName("[]" + elemType)
	if !ok {
		return decodeValue(payload)
	}
	if payload == nil {
		return reflect.Zero(sliceType).Interface()
	}
	rawSlice, ok := payload.([]interface{})
	if !ok {
		return reflect.Zero(sliceType).Interface()
	}
	elem := sliceType.Elem()
	out := reflect.MakeSlice(sliceType, 0, len(rawSlice))
	for _, raw := range rawSlice {
		converted, ok := convertToType(raw, elem)
		if !ok {
			converted = reflect.Zero(elem)
		}
		out = reflect.Append(out, converted)
	}
	return out.Interface()
}

func decodeByteSlice(payload interface{}) []byte {
	if payload == nil {
		return nil
	}
	items, ok := payload.([]interface{})
	if !ok {
		return nil
	}
	out := make([]byte, 0, len(items))
	for _, raw := range items {
		n, ok := asUint64(raw)
		if !ok || n > math.MaxUint8 {
			return nil
		}
		out = append(out, byte(n))
	}
	return out
}

func goTypeFromName(typeName string) (reflect.Type, bool) {
	trimmed := strings.TrimSpace(typeName)
	switch trimmed {
	case "interface {}", "interface{}", "any":
		return interfaceType, true
	case "bool":
		return reflect.TypeOf(bool(false)), true
	case "string":
		return reflect.TypeOf(""), true
	case "int":
		return reflect.TypeOf(int(0)), true
	case "int64":
		return reflect.TypeOf(int64(0)), true
	case "int32", "rune":
		return reflect.TypeOf(int32(0)), true
	case "int16":
		return reflect.TypeOf(int16(0)), true
	case "int8":
		return reflect.TypeOf(int8(0)), true
	case "uint":
		return reflect.TypeOf(uint(0)), true
	case "uint64":
		return reflect.TypeOf(uint64(0)), true
	case "uint32":
		return reflect.TypeOf(uint32(0)), true
	case "uint16":
		return reflect.TypeOf(uint16(0)), true
	case "uint8", "byte":
		return reflect.TypeOf(uint8(0)), true
	case "float64":
		return reflect.TypeOf(float64(0)), true
	case "float32":
		return reflect.TypeOf(float32(0)), true
	}
	if strings.HasPrefix(trimmed, "[]") {
		elemType, ok := goTypeFromName(trimmed[2:])
		if !ok {
			return nil, false
		}
		return reflect.SliceOf(elemType), true
	}
	if strings.HasPrefix(trimmed, "map[string]") {
		elemType, ok := goTypeFromName(trimmed[len("map[string]"):])
		if !ok {
			return nil, false
		}
		return reflect.MapOf(reflect.TypeOf(""), elemType), true
	}
	return nil, false
}

func convertToType(raw interface{}, target reflect.Type) (reflect.Value, bool) {
	value := decodeValue(raw)
	if value == nil {
		return reflect.Zero(target), true
	}

	if target.Kind() == reflect.Interface {
		return reflect.ValueOf(value), true
	}

	switch target.Kind() {
	case reflect.String:
		switch v := value.(type) {
		case string:
			return reflect.ValueOf(v).Convert(target), true
		case []byte:
			return reflect.ValueOf(string(v)).Convert(target), true
		default:
			return reflect.Value{}, false
		}
	case reflect.Bool:
		v, ok := value.(bool)
		if !ok {
			return reflect.Value{}, false
		}
		return reflect.ValueOf(v).Convert(target), true
	case reflect.Int, reflect.Int8, reflect.Int16, reflect.Int32, reflect.Int64:
		n, ok := asInt64(value)
		if !ok {
			return reflect.Value{}, false
		}
		if reflect.Zero(target).OverflowInt(n) {
			return reflect.Value{}, false
		}
		return reflect.ValueOf(n).Convert(target), true
	case reflect.Uint, reflect.Uint8, reflect.Uint16, reflect.Uint32, reflect.Uint64:
		n, ok := asUint64(value)
		if !ok {
			return reflect.Value{}, false
		}
		if reflect.Zero(target).OverflowUint(n) {
			return reflect.Value{}, false
		}
		return reflect.ValueOf(n).Convert(target), true
	case reflect.Float32, reflect.Float64:
		f, ok := asFloat64(value)
		if !ok {
			return reflect.Value{}, false
		}
		if reflect.Zero(target).OverflowFloat(f) {
			return reflect.Value{}, false
		}
		return reflect.ValueOf(f).Convert(target), true
	case reflect.Slice:
		if rv := reflect.ValueOf(value); rv.IsValid() {
			if rv.Type().AssignableTo(target) {
				return rv, true
			}
			if rv.Type().ConvertibleTo(target) {
				return rv.Convert(target), true
			}
		}
		if target.Elem().Kind() == reflect.Uint8 {
			switch v := value.(type) {
			case []byte:
				return reflect.ValueOf(v).Convert(target), true
			case string:
				return reflect.ValueOf([]byte(v)).Convert(target), true
			}
		}
		rawItems, ok := value.([]interface{})
		if !ok {
			return reflect.Value{}, false
		}
		out := reflect.MakeSlice(target, 0, len(rawItems))
		elem := target.Elem()
		for _, item := range rawItems {
			converted, ok := convertToType(item, elem)
			if !ok {
				return reflect.Value{}, false
			}
			out = reflect.Append(out, converted)
		}
		return out, true
	case reflect.Map:
		if rv := reflect.ValueOf(value); rv.IsValid() {
			if rv.Type().AssignableTo(target) {
				return rv, true
			}
			if rv.Type().ConvertibleTo(target) {
				return rv.Convert(target), true
			}
		}
		rawMap, ok := value.(map[string]interface{})
		if !ok {
			return reflect.Value{}, false
		}
		out := reflect.MakeMapWithSize(target, len(rawMap))
		elem := target.Elem()
		for key, item := range rawMap {
			converted, ok := convertToType(item, elem)
			if !ok {
				return reflect.Value{}, false
			}
			out.SetMapIndex(reflect.ValueOf(key), converted)
		}
		return out, true
	}

	converted := reflect.ValueOf(value)
	if converted.IsValid() && converted.Type().ConvertibleTo(target) {
		return converted.Convert(target), true
	}
	return reflect.Value{}, false
}

func asInt64(raw interface{}) (int64, bool) {
	switch v := raw.(type) {
	case int:
		return int64(v), true
	case int64:
		return v, true
	case int32:
		return int64(v), true
	case int16:
		return int64(v), true
	case int8:
		return int64(v), true
	case uint:
		if uint64(v) > math.MaxInt64 {
			return 0, false
		}
		return int64(v), true
	case uint64:
		if v > math.MaxInt64 {
			return 0, false
		}
		return int64(v), true
	case uint32:
		return int64(v), true
	case uint16:
		return int64(v), true
	case uint8:
		return int64(v), true
	case float64:
		if !isWholeFloat(v) || v < math.MinInt64 || v > math.MaxInt64 {
			return 0, false
		}
		return int64(v), true
	case json.Number:
		if i, err := v.Int64(); err == nil {
			return i, true
		}
		if f, err := v.Float64(); err == nil {
			return asInt64(f)
		}
		return 0, false
	default:
		return 0, false
	}
}

func asUint64(raw interface{}) (uint64, bool) {
	switch v := raw.(type) {
	case uint:
		return uint64(v), true
	case uint64:
		return v, true
	case uint32:
		return uint64(v), true
	case uint16:
		return uint64(v), true
	case uint8:
		return uint64(v), true
	case int:
		if v < 0 {
			return 0, false
		}
		return uint64(v), true
	case int64:
		if v < 0 {
			return 0, false
		}
		return uint64(v), true
	case int32:
		if v < 0 {
			return 0, false
		}
		return uint64(v), true
	case int16:
		if v < 0 {
			return 0, false
		}
		return uint64(v), true
	case int8:
		if v < 0 {
			return 0, false
		}
		return uint64(v), true
	case float64:
		if !isWholeFloat(v) || v < 0 || v > math.MaxUint64 {
			return 0, false
		}
		return uint64(v), true
	case json.Number:
		if i, err := v.Int64(); err == nil {
			if i < 0 {
				return 0, false
			}
			return uint64(i), true
		}
		if f, err := v.Float64(); err == nil {
			return asUint64(f)
		}
		return 0, false
	default:
		return 0, false
	}
}

func asFloat64(raw interface{}) (float64, bool) {
	switch v := raw.(type) {
	case float64:
		return v, true
	case float32:
		return float64(v), true
	case int:
		return float64(v), true
	case int64:
		return float64(v), true
	case int32:
		return float64(v), true
	case int16:
		return float64(v), true
	case int8:
		return float64(v), true
	case uint:
		return float64(v), true
	case uint64:
		return float64(v), true
	case uint32:
		return float64(v), true
	case uint16:
		return float64(v), true
	case uint8:
		return float64(v), true
	case json.Number:
		f, err := v.Float64()
		return f, err == nil
	default:
		return 0, false
	}
}

func isWholeFloat(v float64) bool {
	return !math.IsNaN(v) && !math.IsInf(v, 0) && math.Trunc(v) == v
}
