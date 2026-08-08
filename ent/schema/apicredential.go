package schema

import (
	"entgo.io/ent"
	"entgo.io/ent/dialect"
	"entgo.io/ent/schema/edge"
	"entgo.io/ent/schema/field"
	"entgo.io/ent/schema/index"
)

// APICredential stores API keys and pending one-time CLI pairing codes.
type APICredential struct {
	ent.Schema
}

func (APICredential) Fields() []ent.Field {
	return []ent.Field{
		field.Int("user_id"),
		field.String("name").MaxLen(64),
		field.String("public_id").MaxLen(24).Unique(),
		field.Bytes("key_digest").Optional().Sensitive(),
		field.Bytes("pairing_digest").Optional().Sensitive(),
		field.Time("pairing_expires_at").Optional().Nillable().SchemaType(map[string]string{
			dialect.MySQL: "datetime",
		}),
		field.Time("activated_at").Optional().Nillable().SchemaType(map[string]string{
			dialect.MySQL: "datetime",
		}),
		field.Time("last_used_at").Optional().Nillable().SchemaType(map[string]string{
			dialect.MySQL: "datetime",
		}),
		field.Time("expires_at").SchemaType(map[string]string{
			dialect.MySQL: "datetime",
		}),
	}
}

func (APICredential) Edges() []ent.Edge {
	return []ent.Edge{
		edge.From("user", User.Type).
			Field("user_id").
			Ref("api_credentials").
			Unique().
			Required(),
	}
}

func (APICredential) Mixin() []ent.Mixin {
	return []ent.Mixin{CommonMixin{}}
}

func (APICredential) Indexes() []ent.Index {
	return []ent.Index{
		index.Fields("user_id"),
		index.Fields("user_id", "public_id").Unique(),
	}
}
