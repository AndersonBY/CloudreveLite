package manager

import (
	"context"
	"testing"

	"github.com/cloudreve/Cloudreve/v4/ent"
	"github.com/cloudreve/Cloudreve/v4/inventory/types"
	"github.com/stretchr/testify/require"
)

func TestLiteStorageDriverRejectsUnsupportedPolicyTypes(t *testing.T) {
	manager := &manager{}
	for _, policyType := range []types.PolicyType{
		types.PolicyTypeRemote,
		types.PolicyTypeCos,
		types.PolicyTypeKs3,
		types.PolicyTypeObs,
		types.PolicyTypeQiniu,
		types.PolicyTypeUpyun,
		types.PolicyTypeOd,
	} {
		t.Run(string(policyType), func(t *testing.T) {
			_, err := manager.GetStorageDriver(context.Background(), &ent.StoragePolicy{Type: string(policyType)})
			require.Equal(t, ErrUnsupportedPolicyType, err)
		})
	}
}
