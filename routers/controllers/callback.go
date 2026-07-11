package controllers

import (
	"fmt"

	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/fs"
	"github.com/cloudreve/Cloudreve/v4/pkg/filemanager/manager"
	"github.com/cloudreve/Cloudreve/v4/pkg/logging"
	"github.com/cloudreve/Cloudreve/v4/pkg/serializer"
	"github.com/cloudreve/Cloudreve/v4/service/callback"
	"github.com/gin-gonic/gin"
)

// RemoteCallback process callback request to complete upload
func ProcessCallback(failedStatusCode int, generalResp bool) gin.HandlerFunc {
	return func(c *gin.Context) {
		err := callback.ProcessCallback(c)
		if err != nil {
			if generalResp {
				c.JSON(failedStatusCode, serializer.GeneralUploadCallbackFailed{Error: err.Error()})
			} else {
				c.JSON(failedStatusCode, serializer.Err(c, err))
			}
			return
		}

		c.JSON(200, serializer.Response{})
	}
}

// OSSCallbackValidate 阿里云OSS上传回调
func OSSCallbackValidate(c *gin.Context) {
	var callbackBody callback.UploadCallbackService
	if err := c.ShouldBindJSON(&callbackBody); err == nil {
		uploadSession := c.MustGet(manager.UploadSessionCtx).(*fs.UploadSession)
		if uploadSession.Props.Size != callbackBody.Size {
			l := logging.FromContext(c)
			l.Error("Callback validate failed: size mismatch, expected: %d, actual:%d", uploadSession.Props.Size, callbackBody.Size)
			c.JSON(401,
				serializer.GeneralUploadCallbackFailed{
					Error: fmt.Sprintf("size mismatch"),
				})
			c.Abort()
			return
		}

		c.Next()
	} else {
		c.JSON(401, ErrorResponse(err))
		c.Abort()
	}
}

// OneDriveOAuth OneDrive 授权回调
func OneDriveOAuth(c *gin.Context) {
	//var callbackBody callback.OauthService
	//if err := c.ShouldBindQuery(&callbackBody); err == nil {
	//	res := callbackBody.OdAuth(c)
	//	redirect := model.GetSiteURL()
	//	redirect.Path = path.Join(redirect.Path, "/admin/policy")
	//	queries := redirect.Query()
	//	queries.Add("code", strconv.Itoa(res.Code))
	//	queries.Add("msg", res.Msg)
	//	queries.Add("err", res.Error)
	//	redirect.RawQuery = queries.Encode()
	//	c.Redirect(303, redirect.String())
	//} else {
	//	c.JSON(200, ErrorResponse(err))
	//}
}

// GoogleDriveOAuth Google Drive 授权回调
func GoogleDriveOAuth(c *gin.Context) {
	//var callbackBody callback.OauthService
	//if err := c.ShouldBindQuery(&callbackBody); err == nil {
	//	res := callbackBody.GDriveAuth(c)
	//	redirect := model.GetSiteURL()
	//	redirect.Path = path.Join(redirect.Path, "/admin/policy")
	//	queries := redirect.Query()
	//	queries.Add("code", strconv.Itoa(res.Code))
	//	queries.Add("msg", res.Msg)
	//	queries.Add("err", res.Error)
	//	redirect.RawQuery = queries.Encode()
	//	c.Redirect(303, redirect.String())
	//} else {
	//	c.JSON(200, ErrorResponse(err))
	//}
}
